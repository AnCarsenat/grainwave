//! Rect-placed widgets and the auto-fitting cell grid.
//!
//! Every widget here draws inside a rect it is given, so layouts computed from the window size
//! can never overflow. All parameter edits go through `ParamSetter` begin/set/end.

use super::theme::*;
use nih_plug::prelude::*;
use nih_plug_egui::egui::{
    self, pos2, vec2, Align, Align2, Color32, ComboBox, CornerRadius, FontId, Id, Layout, Pos2,
    Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, UiBuilder,
};
use std::f32::consts::PI;

/// Child `Ui` restricted (and clipped) to `rect`. Does not allocate space in the parent.
/// A NaN rect (from layout math on a degenerate window) becomes an empty rect, since egui
/// asserts on NaN `max_rect`.
pub fn child(ui: &mut Ui, rect: Rect, layout: Layout) -> Ui {
    let rect = if rect.any_nan() { Rect::from_min_size(ui.max_rect().min, egui::Vec2::ZERO) } else { rect };
    let mut c = ui.new_child(UiBuilder::new().max_rect(rect).layout(layout));
    c.set_clip_rect(rect.intersect(ui.clip_rect()));
    c
}

fn param_id<P>(ui: &Ui, kind: &str, p: &P) -> Id {
    ui.id().with((kind, p as *const P as usize))
}

// ---------------------------------------------------------------------------
// Knobs
// ---------------------------------------------------------------------------

/// Shared drag logic for knobs and sliders: vertical (or horizontal) drag, Shift for fine,
/// double-click to reset. `span` is the drag distance in pixels for the full range.
fn drag_param<P: Param>(ui: &Ui, resp: &Response, param: &P, s: &ParamSetter, span: f32, horizontal: bool) {
    let drag_id = resp.id.with("drag_value");
    if resp.double_clicked() {
        s.begin_set_parameter(param);
        s.set_parameter(param, param.default_plain_value());
        s.end_set_parameter(param);
        return;
    }
    if resp.drag_started() {
        s.begin_set_parameter(param);
        let start = param.unmodulated_normalized_value();
        ui.memory_mut(|m| m.data.insert_temp(drag_id, start));
    }
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let speed = if fine { 0.25 } else { 1.0 } / span.max(20.0);
        let d = if horizontal { resp.drag_delta().x } else { -resp.drag_delta().y };
        let mut v: f32 = ui
            .memory(|m| m.data.get_temp(drag_id))
            .unwrap_or_else(|| param.unmodulated_normalized_value());
        v = (v + d * speed).clamp(0.0, 1.0);
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

/// Rotary knob filling `rect`: knob on top, label and value below. Drag vertically
/// (Shift: fine), double-click to reset. Bipolar knobs fill from the center.
pub fn knob<P: Param>(ui: &mut Ui, rect: Rect, param: &P, s: &ParamSetter, label: &str, bipolar: bool, color: Color32) -> Response {
    let resp = ui.interact(rect, param_id(ui, "knob", param), Sense::click_and_drag());
    drag_param(ui, &resp, param, s, 160.0, false);

    let text_h = 25.0;
    let d = (rect.height() - text_h - 4.0).min(rect.width() - 8.0).clamp(12.0, 52.0);
    let block_h = d + text_h;
    let top = rect.top() + ((rect.height() - block_h) * 0.5).max(0.0);
    let center = pos2(rect.center().x, top + d * 0.5);
    let painter = ui.painter_at(rect);
    draw_knob(&painter, center, d * 0.5, param.modulated_normalized_value(), bipolar, color, resp.hovered() || resp.dragged());

    let label_y = top + d + 2.0;
    painter.text(pos2(rect.center().x, label_y), Align2::CENTER_TOP, label, FontId::proportional(10.5), TEXT);
    painter.text(pos2(rect.center().x, label_y + 12.5), Align2::CENTER_TOP, param.to_string(), FontId::proportional(9.5), TEXT_DIM);
    resp.on_hover_text(format!("{}: {}\nDrag (Shift: fine), double-click to reset", param.name(), param))
}

/// Knob with label and value to its right, for short rows (top bar).
pub fn knob_compact<P: Param>(ui: &mut Ui, rect: Rect, param: &P, s: &ParamSetter, label: &str, color: Color32) -> Response {
    let resp = ui.interact(rect, param_id(ui, "knobc", param), Sense::click_and_drag());
    drag_param(ui, &resp, param, s, 160.0, false);
    let d = (rect.height() - 4.0).clamp(12.0, 34.0);
    let center = pos2(rect.left() + d * 0.5 + 2.0, rect.center().y);
    let painter = ui.painter_at(rect);
    draw_knob(&painter, center, d * 0.5, param.modulated_normalized_value(), false, color, resp.hovered() || resp.dragged());
    let tx = rect.left() + d + 6.0;
    painter.text(pos2(tx, rect.center().y - 1.0), Align2::LEFT_BOTTOM, label, FontId::proportional(10.5), TEXT);
    painter.text(pos2(tx, rect.center().y + 1.0), Align2::LEFT_TOP, param.to_string(), FontId::proportional(9.5), TEXT_DIM);
    resp.on_hover_text(format!("{}: {}\nDrag (Shift: fine), double-click to reset", param.name(), param))
}

fn draw_knob(painter: &egui::Painter, center: Pos2, radius: f32, norm: f32, bipolar: bool, color: Color32, hot: bool) {
    let start = PI * 0.75; // 7:30 position (egui y points down)
    let sweep = PI * 1.5;
    let norm = if norm.is_finite() { norm.clamp(0.0, 1.0) } else { 0.0 };
    let from: f32 = if bipolar { 0.5 } else { 0.0 };
    let w = (radius * 0.18).clamp(2.0, 4.0);
    painter.circle_filled(center, radius - w, KNOB_BODY);
    arc(painter, center, radius - w * 0.5, start, start + sweep, Stroke::new(w, KNOB_TRACK));
    let (a0, a1) = (start + sweep * from.min(norm), start + sweep * from.max(norm));
    if a1 - a0 > 0.001 {
        let c = if hot { color.gamma_multiply(1.25) } else { color };
        arc(painter, center, radius - w * 0.5, a0, a1, Stroke::new(w, c));
    }
    let a = start + sweep * norm;
    let dir = vec2(a.cos(), a.sin());
    painter.line_segment([center + dir * (radius * 0.2), center + dir * (radius - w - 1.0)], Stroke::new(2.0_f32, TEXT));
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

// ---------------------------------------------------------------------------
// Sliders, toggles, combos
// ---------------------------------------------------------------------------

/// Fader for a 0..1-ish parameter. Vertical fills from the bottom, horizontal from the left.
/// Drag along the fader (Shift: fine), double-click to reset.
pub fn slider<P: Param>(ui: &mut Ui, rect: Rect, param: &P, s: &ParamSetter, color: Color32, vertical: bool) -> Response {
    let resp = ui.interact(rect, param_id(ui, "slider", param), Sense::click_and_drag());
    let span = if vertical { rect.height() } else { rect.width() };
    drag_param(ui, &resp, param, s, span, !vertical);
    let painter = ui.painter_at(rect);
    let norm = param.modulated_normalized_value().clamp(0.0, 1.0);
    painter.rect_filled(rect, 3.0, WELL);
    let fill = if vertical {
        Rect::from_min_max(pos2(rect.left(), rect.bottom() - rect.height() * norm), rect.max)
    } else {
        Rect::from_min_max(rect.min, pos2(rect.left() + rect.width() * norm, rect.bottom()))
    };
    let c = if resp.hovered() || resp.dragged() { color } else { color.gamma_multiply(0.8) };
    painter.rect_filled(fill, 3.0, c);
    painter.rect_stroke(rect, 3.0, Stroke::new(1.0_f32, PANEL_EDGE), StrokeKind::Inside);
    resp.on_hover_text(format!("{}: {}\nDrag (Shift: fine), double-click to reset", param.name(), param))
}

/// Toggle button for a bool parameter, centered in `rect`.
pub fn toggle(ui: &mut Ui, rect: Rect, param: &BoolParam, s: &ParamSetter, label: &str, color: Color32) -> Response {
    let on = param.value();
    let h = rect.height().min(20.0);
    let r = Rect::from_center_size(rect.center(), vec2(rect.width(), h));
    let resp = ui.interact(r, param_id(ui, "toggle", param), Sense::click());
    if resp.clicked() {
        s.begin_set_parameter(param);
        s.set_parameter(param, !on);
        s.end_set_parameter(param);
    }
    let painter = ui.painter_at(rect);
    let fill = if on { dim(color, 0.35) } else if resp.hovered() { Color32::from_rgb(0x45, 0x3c, 0x34) } else { Color32::from_rgb(0x33, 0x2d, 0x28) };
    painter.rect_filled(r, CornerRadius::same(4), fill);
    painter.rect_stroke(r, CornerRadius::same(4), Stroke::new(1.0_f32, if on { color } else { PANEL_EDGE }), StrokeKind::Inside);
    painter.text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(11.0), if on { color } else { TEXT });
    resp.on_hover_text(param.name())
}

/// Power switch: LED plus the unit name. Clicking toggles the parameter.
pub fn power(ui: &mut Ui, rect: Rect, param: &BoolParam, s: &ParamSetter, label: &str, color: Color32) -> Response {
    let on = param.value();
    let resp = ui.interact(rect, param_id(ui, "power", param), Sense::click());
    if resp.clicked() {
        s.begin_set_parameter(param);
        s.set_parameter(param, !on);
        s.end_set_parameter(param);
    }
    let painter = ui.painter_at(rect);
    if resp.hovered() {
        painter.rect_filled(rect, 3.0, Color32::from_rgb(0x30, 0x2b, 0x27));
    }
    let led = pos2(rect.left() + 8.0, rect.center().y);
    if on {
        painter.circle_filled(led, 6.0, color.gamma_multiply(0.25));
        painter.circle_filled(led, 4.0, color);
    } else {
        painter.circle_filled(led, 4.0, KNOB_TRACK);
        painter.circle_stroke(led, 4.0, Stroke::new(1.0_f32, TEXT_DIM));
    }
    painter.text(
        pos2(rect.left() + 17.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.0),
        if on { color } else { TEXT_DIM },
    );
    resp.on_hover_text(format!("{} (click to switch {})", param.name(), if on { "off" } else { "on" }))
}

/// Combo box for a stepped parameter (enums). Label above when there is room, else to the left.
pub fn combo<P: Param>(ui: &mut Ui, rect: Rect, param: &P, s: &ParamSetter, label: &str) {
    let steps = param.step_count().unwrap_or(1).max(1);
    let current = (param.unmodulated_normalized_value() * steps as f32).round() as usize;
    let stacked = rect.height() >= 36.0;
    let (label_rect, box_rect) = if stacked {
        let top = rect.center().y - 17.0;
        (
            Rect::from_min_size(pos2(rect.left() + 2.0, top), vec2(rect.width() - 4.0, 14.0)),
            Rect::from_min_size(pos2(rect.left() + 2.0, top + 15.0), vec2(rect.width() - 4.0, 19.0)),
        )
    } else {
        let lw = (rect.width() * 0.38).min(70.0);
        (
            Rect::from_min_size(rect.min, vec2(lw, rect.height())),
            Rect::from_min_max(pos2(rect.left() + lw, rect.top()), rect.max),
        )
    };
    let align = if stacked { Align2::CENTER_CENTER } else { Align2::LEFT_CENTER };
    let lpos = if stacked { label_rect.center() } else { label_rect.left_center() };
    ui.painter_at(rect).text(lpos, align, label, FontId::proportional(10.5), TEXT);
    let mut c = child(ui, box_rect, Layout::left_to_right(Align::Center));
    ComboBox::from_id_salt(param_id(ui, "combo", param))
        .width(box_rect.width() - 2.0)
        .height(320.0)
        .selected_text(egui::RichText::new(param.to_string()).size(11.0))
        .show_ui(&mut c, |ui| {
            for i in 0..=steps {
                let n = i as f32 / steps as f32;
                let text = param.normalized_value_to_string(n, false);
                if ui.selectable_label(i == current, text).clicked() && i != current {
                    s.begin_set_parameter(param);
                    s.set_parameter(param, param.preview_plain(n));
                    s.end_set_parameter(param);
                }
            }
        })
        .response
        .on_hover_text(param.name());
}

/// Momentary button: on while the pointer is held on it, off on release. `held` remembers
/// whether this editor started the gesture so begin/end stay balanced; `drawn` is set so the
/// editor can release the parameter when the button stops being drawn (tab switch).
pub fn momentary(ui: &mut Ui, rect: Rect, param: &BoolParam, s: &ParamSetter, label: &str, held: &mut bool, drawn: &mut bool) {
    *drawn = true;
    let resp = ui.interact(rect, param_id(ui, "momentary", param), Sense::click_and_drag());
    let down = resp.is_pointer_button_down_on();
    if down && !*held {
        s.begin_set_parameter(param);
        s.set_parameter(param, true);
        *held = true;
    } else if !down && *held {
        release_momentary(param, s, held);
    }
    let on = param.value();
    let painter = ui.painter_at(rect);
    let fill = if on { HOT } else if resp.hovered() { Color32::from_rgb(0x45, 0x3c, 0x34) } else { Color32::from_rgb(0x33, 0x2d, 0x28) };
    painter.rect_filled(rect, 6.0, fill);
    painter.rect_stroke(rect, 6.0, Stroke::new(1.0_f32, if on { HOT } else { PANEL_EDGE }), StrokeKind::Inside);
    painter.text(rect.center(), Align2::CENTER_CENTER, label, FontId::proportional(13.0), if on { BG } else { TEXT });
    resp.on_hover_text("Hold");
}

/// End a momentary gesture started by this editor.
pub fn release_momentary(param: &BoolParam, s: &ParamSetter, held: &mut bool) {
    if *held {
        s.set_parameter(param, false);
        s.end_set_parameter(param);
        *held = false;
    }
}

/// Horizontal or vertical meter. Bipolar meters (value -1..1) fill from the center.
pub fn meter(painter: &egui::Painter, rect: Rect, value: f32, bipolar: bool, color: Color32) {
    painter.rect_filled(rect, 2.0, WELL);
    let v = if value.is_finite() { value } else { 0.0 };
    let vertical = rect.height() > rect.width();
    let fill = if vertical {
        if bipolar {
            let c = rect.center().y;
            let y = c - v.clamp(-1.0, 1.0) * rect.height() * 0.5;
            Rect::from_x_y_ranges(rect.x_range(), c.min(y)..=c.max(y))
        } else {
            Rect::from_x_y_ranges(rect.x_range(), rect.bottom() - v.clamp(0.0, 1.0) * rect.height()..=rect.bottom())
        }
    } else if bipolar {
        let c = rect.center().x;
        let x = c + v.clamp(-1.0, 1.0) * rect.width() * 0.5;
        Rect::from_x_y_ranges(c.min(x)..=c.max(x), rect.y_range())
    } else {
        Rect::from_x_y_ranges(rect.left()..=rect.left() + v.clamp(0.0, 1.0) * rect.width(), rect.y_range())
    };
    painter.rect_filled(fill, 2.0, color);
}

/// Panel frame with a colored title. Returns the content rect below the title.
pub fn group_box(ui: &Ui, rect: Rect, title: &str, color: Color32) -> Rect {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 5.0, PANEL);
    painter.rect_stroke(rect, 5.0, Stroke::new(1.0_f32, PANEL_EDGE), StrokeKind::Inside);
    painter.rect_filled(Rect::from_min_size(rect.min + vec2(PAD, 6.0), vec2(3.0, 10.0)), 1.0, color);
    painter.text(rect.min + vec2(PAD + 7.0, 4.0), Align2::LEFT_TOP, title, FontId::proportional(11.0), color);
    Rect::from_min_max(rect.min + vec2(PAD, 19.0), rect.max - vec2(PAD, PAD))
}

// ---------------------------------------------------------------------------
// Auto-fitting cell grid
// ---------------------------------------------------------------------------

type CellFn<'a> = Box<dyn FnOnce(&mut Ui, Rect) + 'a>;

/// A list of controls laid out on a grid that always fits its rect: the column count is chosen
/// so cells stay as close as possible to the preferred size, and cells shrink when space is
/// short instead of overflowing. Each item spans 1 or more columns (combos use 2).
pub struct Cells<'a> {
    items: Vec<(usize, CellFn<'a>)>,
    color: Color32,
}

pub const CELL_PREF: egui::Vec2 = egui::Vec2::new(58.0, 66.0);
pub const CELL_MAX: egui::Vec2 = egui::Vec2::new(86.0, 92.0);

impl<'a> Cells<'a> {
    pub fn new(color: Color32) -> Self {
        Self { items: Vec::new(), color }
    }

    pub fn add(&mut self, units: usize, f: impl FnOnce(&mut Ui, Rect) + 'a) -> &mut Self {
        self.items.push((units.max(1), Box::new(f)));
        self
    }

    pub fn knob<P: Param>(&mut self, s: &'a ParamSetter<'a>, p: &'a P, label: &'a str) -> &mut Self {
        let c = self.color;
        self.add(1, move |ui, r| {
            knob(ui, r, p, s, label, false, c);
        })
    }

    pub fn knob_bi<P: Param>(&mut self, s: &'a ParamSetter<'a>, p: &'a P, label: &'a str) -> &mut Self {
        let c = self.color;
        self.add(1, move |ui, r| {
            knob(ui, r, p, s, label, true, c);
        })
    }

    pub fn combo<P: Param>(&mut self, s: &'a ParamSetter<'a>, p: &'a P, label: &'a str) -> &mut Self {
        self.add(2, move |ui, r| combo(ui, r.shrink2(vec2(2.0, 0.0)), p, s, label))
    }

    pub fn toggle(&mut self, s: &'a ParamSetter<'a>, p: &'a BoolParam, label: &'a str) -> &mut Self {
        let c = self.color;
        self.add(1, move |ui, r| {
            toggle(ui, r.shrink2(vec2(2.0, 0.0)), p, s, label, c);
        })
    }

    pub fn units(&self) -> usize {
        self.items.iter().map(|(u, _)| *u).sum()
    }

    pub fn show(self, ui: &mut Ui, rect: Rect) {
        let units: Vec<usize> = self.items.iter().map(|(u, _)| *u).collect();
        let rects = fit_cells(rect, &units, CELL_PREF, CELL_MAX);
        for ((_, f), r) in self.items.into_iter().zip(rects) {
            f(ui, r);
        }
    }
}

/// Grid placement for items of the given widths (in columns). Pure function, unit tested.
pub fn fit_cells(rect: Rect, units: &[usize], pref: egui::Vec2, max: egui::Vec2) -> Vec<Rect> {
    if units.is_empty() || rect.width() <= 0.0 || rect.height() <= 0.0 {
        // Empty but finite: `Rect::NOTHING` (infinite corners) turns into NaN as soon as a
        // widget shrinks it, which makes egui panic.
        return vec![Rect::from_min_size(rect.min, egui::Vec2::ZERO); units.len()];
    }
    let widest = units.iter().copied().max().unwrap_or(1);
    let total: usize = units.iter().sum();
    let rows_for = |cols: usize| {
        let mut rows = 1;
        let mut used = 0;
        for &u in units {
            if used + u > cols {
                rows += 1;
                used = 0;
            }
            used += u;
        }
        rows
    };
    let mut best = (widest, rows_for(widest), f32::MIN);
    for cols in widest..=total {
        let rows = rows_for(cols);
        let cw = rect.width() / cols as f32;
        let ch = rect.height() / rows as f32;
        let score = (cw / pref.x).min(ch / pref.y).min(1.0);
        if score > best.2 + 1e-4 {
            best = (cols, rows, score);
        }
    }
    let (cols, rows, _) = best;
    let cw = (rect.width() / cols as f32).min(max.x);
    let ch = (rect.height() / rows as f32).min(max.y);
    let mut out = Vec::with_capacity(units.len());
    let (mut col, mut row) = (0usize, 0usize);
    for &u in units {
        if col + u > cols {
            row += 1;
            col = 0;
        }
        let min = pos2(rect.left() + col as f32 * cw, rect.top() + row as f32 * ch);
        out.push(Rect::from_min_size(min, vec2(cw * u as f32, ch)));
        col += u;
    }
    out
}

/// Row of titled boxes, widths proportional to their content. Each box auto-fits its cells.
pub fn boxes<'a>(ui: &mut Ui, rect: Rect, list: Vec<(&str, Color32, Cells<'a>)>) {
    let n = list.len();
    if n == 0 {
        return;
    }
    let weights: Vec<f32> = list.iter().map(|(_, _, c)| c.units().max(2) as f32).collect();
    let total: f32 = weights.iter().sum();
    let free = rect.width() - GAP * (n - 1) as f32;
    let mut x = rect.left();
    for ((title, color, cells), w) in list.into_iter().zip(weights) {
        let width = free * w / total;
        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(width, rect.height()));
        let inner = group_box(ui, r, title, color);
        cells.show(ui, inner);
        x += width + GAP;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_in_an_empty_rect_are_finite() {
        let rect = Rect::from_min_max(pos2(10.0, 20.0), pos2(5.0, 18.0));
        for r in fit_cells(rect, &[1, 2, 1], CELL_PREF, CELL_MAX) {
            assert!(r.min.x.is_finite() && r.max.y.is_finite(), "{r:?}");
            assert!(!r.shrink2(vec2(2.0, 0.0)).any_nan(), "{r:?} turns into NaN when shrunk");
        }
    }

    #[test]
    fn cells_always_fit() {
        let units = [1, 2, 1, 1, 1, 2, 1, 1, 1, 1, 1, 1, 2, 1, 1, 1, 1];
        for (w, h) in [(800.0, 300.0), (200.0, 120.0), (90.0, 400.0), (600.0, 60.0)] {
            let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(w, h));
            let rects = fit_cells(rect, &units, CELL_PREF, CELL_MAX);
            assert_eq!(rects.len(), units.len());
            for r in &rects {
                assert!(rect.expand(0.01).contains_rect(*r), "{r:?} outside {rect:?}");
            }
            for (i, a) in rects.iter().enumerate() {
                for b in &rects[i + 1..] {
                    assert!(!a.shrink(0.01).intersects(b.shrink(0.01)), "overlap {a:?} {b:?}");
                }
            }
        }
    }
}
