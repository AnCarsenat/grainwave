//! Time shaper curve editor, shared by the Simple and Advanced pages.
//!
//! The editor always shows the pattern selected by `ts_pat` (the "viewed" pattern). The
//! pattern actually playing (`ts_active`, after quantized switching) is lit in the button row.
//!
//! Mouse:
//! - click empty space: add a point (drag right away to place it)
//! - drag a point: move it (x stays between its neighbours); Alt disables grid snap
//! - Shift + drag a point vertically: tension of the segment starting at that point
//! - right-click a point: cycle its segment shape Curve -> S-curve -> Hold
//! - double-click a point: delete it (at least one point always remains)
//!
//! Every edit: `time_patterns.write()`, change the viewed pattern, `sanitize()`, release the
//! lock, `pattern_version.fetch_add(1)`. The audio thread picks the change up with `try_read`.

use super::theme::*;
use super::widgets::child;
use crate::params::GrainwaveParams;
use crate::pattern::{self, Pattern, PatternBank, Point, SegmentKind, MAX_POINTS, NUM_PATTERNS};
use crate::shared::{load_f32, VisualState};
use nih_plug::prelude::*;
use nih_plug_egui::egui::{
    self, pos2, vec2, Align, Align2, Color32, FontId, Id, Layout, Pos2, Rect, Sense, Shape, Stroke,
    StrokeKind, Ui,
};
use std::sync::atomic::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Snap {
    Off,
    Eighth,
    Twelfth,
    Sixteenth,
}

impl Snap {
    pub const ALL: [Snap; 4] = [Snap::Off, Snap::Eighth, Snap::Twelfth, Snap::Sixteenth];

    pub fn divisions(self) -> Option<f32> {
        match self {
            Snap::Off => None,
            Snap::Eighth => Some(8.0),
            Snap::Twelfth => Some(12.0),
            Snap::Sixteenth => Some(16.0),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Snap::Off => "Off",
            Snap::Eighth => "1/8",
            Snap::Twelfth => "1/12",
            Snap::Sixteenth => "1/16",
        }
    }
}

pub struct CurveState {
    /// Point being dragged and whether the drag edits tension (Shift held at drag start).
    drag: Option<usize>,
    tension_drag: bool,
    tension_start: f32,
    drag_y0: f32,
    pub snap: Snap,
    name_buf: String,
}

impl Default for CurveState {
    fn default() -> Self {
        Self {
            drag: None,
            tension_drag: false,
            tension_start: 0.0,
            drag_y0: 0.0,
            snap: Snap::Sixteenth,
            name_buf: String::new(),
        }
    }
}

const HELP: &str = "Click empty space: add a point\n\
Drag a point: move it (Alt: no snap)\n\
Shift + drag a point: bend the segment (tension)\n\
Right-click a point: Curve / S-curve / Hold\n\
Double-click a point: delete it\n\n\
Height = how far behind live playback reads.\n\
Bottom = live, a rising line = slow down / hold,\n\
steps up = stutter repeats.";

/// Copy of the current bank (GUI thread, a few hundred bytes).
pub fn read_bank(params: &GrainwaveParams) -> PatternBank {
    params.time_patterns.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Edit one pattern: write lock, change, sanitize, release, bump the version.
pub fn edit_pattern(params: &GrainwaveParams, idx: usize, f: impl FnOnce(&mut Pattern)) {
    {
        let mut bank = params.time_patterns.write().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = bank.patterns.get_mut(idx) {
            f(p);
            p.sanitize();
        }
    }
    params.pattern_version.fetch_add(1, Ordering::Relaxed);
}

/// Index of the viewed pattern (`ts_pat` - 1).
pub fn viewed_index(params: &GrainwaveParams) -> usize {
    (params.time.pattern.value() - 1).clamp(0, NUM_PATTERNS as i32 - 1) as usize
}

/// Pattern buttons + tool row + canvas, filling `rect`. `title_w` reserves room at the left of
/// the button row for a caller-drawn title.
pub fn curve_panel(
    ui: &mut Ui,
    rect: Rect,
    title_w: f32,
    params: &GrainwaveParams,
    visual: &VisualState,
    s: &ParamSetter,
    state: &mut CurveState,
) {
    let row_h = 22.0;
    let tools_h = 20.0;
    let buttons = Rect::from_min_max(pos2(rect.left() + title_w, rect.top()), pos2(rect.right(), rect.top() + row_h));
    let tools = Rect::from_min_size(pos2(rect.left(), buttons.bottom() + 4.0), vec2(rect.width(), tools_h));
    let canvas = Rect::from_min_max(pos2(rect.left(), tools.bottom() + 4.0), rect.max);

    let bank = read_bank(params);
    pattern_buttons(ui, buttons, params, visual, s, &bank);
    let idx = viewed_index(params);
    tool_row(ui, tools, params, state, idx, &bank);
    if canvas.height() > 20.0 {
        curve_canvas(ui, canvas, params, visual, state, idx);
    }
}

fn pattern_buttons(ui: &mut Ui, rect: Rect, params: &GrainwaveParams, visual: &VisualState, s: &ParamSetter, bank: &PatternBank) {
    let gap = 3.0;
    let n = NUM_PATTERNS as f32;
    let w = ((rect.width() - gap * (n - 1.0)) / n).max(10.0);
    let viewed = viewed_index(params);
    let active = visual.ts_active.load(Ordering::Relaxed) as usize;
    let tension = params.time.tension.value();
    for i in 0..NUM_PATTERNS {
        let r = Rect::from_min_size(pos2(rect.left() + i as f32 * (w + gap), rect.top()), vec2(w, rect.height()));
        let resp = ui.interact(r, Id::new(("ts_pattern_button", i)), Sense::click());
        if resp.clicked() && i != viewed {
            let p = &params.time.pattern;
            s.begin_set_parameter(p);
            s.set_parameter(p, i as i32 + 1);
            s.end_set_parameter(p);
        }
        let painter = ui.painter_at(r);
        let is_active = i == active;
        let fill = if is_active {
            dim(C_TIME, 0.32)
        } else if resp.hovered() {
            Color32::from_rgb(0x3a, 0x33, 0x2d)
        } else {
            Color32::from_rgb(0x2c, 0x27, 0x23)
        };
        painter.rect_filled(r, 3.0, fill);
        let stroke = if i == viewed { Stroke::new(1.5_f32, C_TIME) } else { Stroke::new(1.0_f32, PANEL_EDGE) };
        painter.rect_stroke(r, 3.0, stroke, StrokeKind::Inside);
        let num_w = if w > 40.0 { 15.0 } else { w };
        painter.text(
            pos2(r.left() + num_w * 0.5 + 2.0, r.center().y),
            Align2::CENTER_CENTER,
            format!("{}", i + 1),
            FontId::proportional(10.5),
            if i == viewed { C_TIME } else { TEXT },
        );
        // Sparkline of the pattern.
        if w > 40.0 {
            if let Some(pat) = bank.patterns.get(i) {
                let sp = Rect::from_min_max(pos2(r.left() + num_w + 4.0, r.top() + 4.0), pos2(r.right() - 4.0, r.bottom() - 4.0));
                let pts: Vec<Pos2> = (0..=24)
                    .map(|k| {
                        let x = k as f32 / 24.0;
                        let y = pat.value_at(x.min(0.9999), tension);
                        pos2(sp.left() + x * sp.width(), sp.bottom() - y * sp.height())
                    })
                    .collect();
                let c = if is_active { C_TIME } else { TEXT_DIM };
                painter.add(Shape::line(pts, Stroke::new(1.0_f32, c)));
            }
        }
        let name = bank.patterns.get(i).map(|p| p.name.as_str()).unwrap_or("");
        let mut tip = format!("Pattern {}: {name}", i + 1);
        if is_active {
            tip.push_str("\n(playing)");
        }
        resp.on_hover_text(tip);
    }
}

fn tool_row(ui: &mut Ui, rect: Rect, params: &GrainwaveParams, state: &mut CurveState, idx: usize, bank: &PatternBank) {
    let mut c = child(ui, rect, Layout::left_to_right(Align::Center));
    c.spacing_mut().item_spacing.x = 4.0;
    c.label(egui::RichText::new(format!("{}", idx + 1)).color(C_TIME).size(11.0));
    let te_id = Id::new("grainwave_ts_name");
    let focused = c.memory(|m| m.has_focus(te_id));
    if !focused {
        state.name_buf = bank.patterns.get(idx).map(|p| p.name.clone()).unwrap_or_default();
    }
    let name_w = (rect.width() * 0.22).clamp(80.0, 170.0);
    let r = c.add(
        egui::TextEdit::singleline(&mut state.name_buf)
            .id(te_id)
            .desired_width(name_w)
            .char_limit(24)
            .font(FontId::proportional(11.0)),
    );
    if r.changed() {
        let name = state.name_buf.clone();
        edit_pattern(params, idx, |p| p.name = name);
    }
    if c
        .add(egui::Button::new(egui::RichText::new("Reset").size(10.5)))
        .on_hover_text("Restore this slot's factory curve")
        .clicked()
    {
        if let Some(factory) = pattern::factory_bank().patterns.get(idx).cloned() {
            edit_pattern(params, idx, |p| *p = factory);
        }
    }
    c.add_space(8.0);
    c.label(egui::RichText::new("Snap").size(10.5).color(TEXT_DIM));
    for snap in Snap::ALL {
        if c
            .add(egui::Button::new(egui::RichText::new(snap.label()).size(10.5)).selected(state.snap == snap))
            .on_hover_text("Grid for placing points (x and y). 1/12 = triplet steps.")
            .clicked()
        {
            state.snap = snap;
        }
    }
    let n_points = bank.patterns.get(idx).map_or(0, |p| p.points.len());
    c.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.label(egui::RichText::new(" ? ").color(C_TIME).strong()).on_hover_text(HELP);
        ui.label(egui::RichText::new(format!("{n_points}/{MAX_POINTS} pts")).size(10.0).color(TEXT_DIM));
    });
}

fn curve_canvas(ui: &mut Ui, rect: Rect, params: &GrainwaveParams, visual: &VisualState, state: &mut CurveState, idx: usize) {
    let resp = ui.interact(rect, Id::new("grainwave_ts_canvas"), Sense::click_and_drag());
    let inner = rect.shrink2(vec2(8.0, 8.0));
    let to_screen = |x: f32, y: f32| pos2(inner.left() + x * inner.width(), inner.bottom() - y * inner.height());
    let from_screen = |p: Pos2| {
        (
            ((p.x - inner.left()) / inner.width()).clamp(0.0, 1.0),
            ((inner.bottom() - p.y) / inner.height()).clamp(0.0, 1.0),
        )
    };
    let (shift, alt) = ui.input(|i| (i.modifiers.shift, i.modifiers.alt));
    let divisions = if alt { None } else { state.snap.divisions() };
    let snap = |v: f32| match divisions {
        Some(d) => (v * d).round() / d,
        None => v,
    };

    // --- Interaction (on the live bank; the drawing below re-reads it) ---
    let points_now = read_bank(params).patterns.get(idx).map(|p| p.points.clone()).unwrap_or_default();
    let hit = |p: Pos2| -> Option<usize> {
        points_now
            .iter()
            .enumerate()
            .map(|(i, q)| (i, to_screen(q.x, q.y).distance(p)))
            .filter(|(_, d)| *d <= 9.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };

    if resp.drag_started() {
        let origin = ui.input(|i| i.pointer.press_origin()).or(resp.interact_pointer_pos());
        if let Some(o) = origin {
            match hit(o) {
                Some(i) => {
                    state.drag = Some(i);
                    state.tension_drag = shift;
                    state.tension_start = points_now[i].tension;
                    state.drag_y0 = o.y;
                }
                None => {
                    let (x, y) = from_screen(o);
                    state.drag = add_point(params, idx, snap(x), snap(y));
                    state.tension_drag = false;
                }
            }
        }
    }
    if resp.dragged() {
        if let (Some(i), Some(p)) = (state.drag, resp.interact_pointer_pos()) {
            if state.tension_drag {
                let t = (state.tension_start + (state.drag_y0 - p.y) * 0.01).clamp(-1.0, 1.0);
                edit_pattern(params, idx, |pat| {
                    if let Some(q) = pat.points.get_mut(i) {
                        q.tension = t;
                    }
                });
            } else {
                let (x, y) = from_screen(p);
                let (x, y) = (snap(x), snap(y));
                edit_pattern(params, idx, |pat| move_point(&mut pat.points, i, x, y));
            }
        }
    }
    if resp.drag_stopped() {
        state.drag = None;
    }
    let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
    if resp.double_clicked() {
        if let Some(i) = pointer.and_then(hit) {
            edit_pattern(params, idx, |pat| {
                if pat.points.len() > 1 && i < pat.points.len() {
                    pat.points.remove(i);
                }
            });
        }
    } else if resp.clicked() {
        if let Some(p) = pointer {
            if hit(p).is_none() {
                let (x, y) = from_screen(p);
                add_point(params, idx, snap(x), snap(y));
            }
        }
    }
    if resp.secondary_clicked() {
        if let Some(i) = pointer.and_then(hit) {
            edit_pattern(params, idx, |pat| {
                if let Some(q) = pat.points.get_mut(i) {
                    q.kind = match q.kind {
                        SegmentKind::Curve => SegmentKind::SCurve,
                        SegmentKind::SCurve => SegmentKind::Hold,
                        SegmentKind::Hold => SegmentKind::Curve,
                    };
                }
            });
        }
    }

    // --- Drawing ---
    let pat = read_bank(params).patterns.get(idx).cloned().unwrap_or_else(|| Pattern::flat(""));
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, WELL);

    // Grid: snap divisions (1/16 when off), beat lines every quarter, horizontal quarters.
    let div = state.snap.divisions().unwrap_or(16.0) as usize;
    for k in 1..div {
        let x = k as f32 / div as f32;
        let quarter = (x * 4.0 - (x * 4.0).round()).abs() < 1e-4;
        let c = if quarter { GRID_LINE_STRONG } else { GRID_LINE };
        painter.line_segment([to_screen(x, 0.0), to_screen(x, 1.0)], Stroke::new(1.0_f32, c));
    }
    for k in 1..4 {
        let y = k as f32 / 4.0;
        painter.line_segment([to_screen(0.0, y), to_screen(1.0, y)], Stroke::new(1.0_f32, GRID_LINE));
    }
    painter.line_segment([to_screen(0.0, 0.0), to_screen(1.0, 0.0)], Stroke::new(1.0_f32, GRID_LINE_STRONG));

    // Curve: sampled `value_at` with the global tension, filled underneath.
    let tension = params.time.tension.value();
    let n = (inner.width() / 2.0).clamp(32.0, 600.0) as usize;
    let samples: Vec<Pos2> = (0..=n)
        .map(|k| {
            let x = k as f32 / n as f32;
            to_screen(x, pat.value_at(x.min(0.99999), tension))
        })
        .collect();
    let fill = C_TIME.gamma_multiply(0.18);
    let base = inner.bottom();
    for w in samples.windows(2) {
        painter.add(Shape::convex_polygon(
            vec![w[0], w[1], pos2(w[1].x, base), pos2(w[0].x, base)],
            fill,
            Stroke::NONE,
        ));
    }
    painter.add(Shape::line(samples, Stroke::new(2.0_f32, C_TIME)));

    // Playback cursor (bright when the viewed pattern is the one playing).
    let active = visual.ts_active.load(Ordering::Relaxed) as usize == idx;
    let cur = load_f32(&visual.ts_cursor);
    if cur.is_finite() && params.sections.time.value() {
        let x = cur.clamp(0.0, 1.0);
        let c = if active { TEXT } else { TEXT_DIM.gamma_multiply(0.5) };
        painter.line_segment([to_screen(x, 0.0), to_screen(x, 1.0)], Stroke::new(1.0_f32, c));
        let v = load_f32(&visual.ts_value);
        if active && v.is_finite() {
            painter.circle_filled(to_screen(x, v.clamp(0.0, 1.0)), 4.0, TEXT);
        }
    }

    // Points: circle = curve, diamond = S-curve, square = hold.
    let hover = resp.hover_pos().and_then(|p| {
        pat.points
            .iter()
            .enumerate()
            .map(|(i, q)| (i, to_screen(q.x, q.y).distance(p)))
            .filter(|(_, d)| *d <= 9.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    });
    for (i, q) in pat.points.iter().enumerate() {
        let c = to_screen(q.x, q.y);
        let hot = hover == Some(i) || state.drag == Some(i);
        let r = if hot { 6.0 } else { 4.5 };
        let fill = if hot { TEXT } else { C_TIME };
        draw_point(&painter, c, r, q.kind, fill);
    }
    if let Some(i) = state.drag.filter(|_| state.tension_drag) {
        if let Some(q) = pat.points.get(i) {
            let c = to_screen(q.x, q.y);
            painter.text(c + vec2(8.0, -8.0), Align2::LEFT_BOTTOM, format!("tension {:+.2}", q.tension), FontId::proportional(10.0), TEXT);
        }
    } else if let Some(i) = hover {
        if let Some(q) = pat.points.get(i) {
            let kind = match q.kind {
                SegmentKind::Curve => "curve",
                SegmentKind::SCurve => "S-curve",
                SegmentKind::Hold => "hold",
            };
            let c = to_screen(q.x, q.y);
            let anchor = if q.x > 0.8 { Align2::RIGHT_BOTTOM } else { Align2::LEFT_BOTTOM };
            let off = if q.x > 0.8 { vec2(-8.0, -6.0) } else { vec2(8.0, -6.0) };
            painter.text(c + off, anchor, format!("{kind} · x {:.3} · y {:.3}", q.x, q.y), FontId::proportional(10.0), TEXT_DIM);
        }
    }

    painter.text(inner.left_bottom() + vec2(2.0, -2.0), Align2::LEFT_BOTTOM, "live", FontId::proportional(9.5), TEXT_DIM);
    painter.text(inner.left_top() + vec2(2.0, 2.0), Align2::LEFT_TOP, "1 cycle back", FontId::proportional(9.5), TEXT_DIM);
    painter.rect_stroke(rect, 4.0, Stroke::new(1.0_f32, PANEL_EDGE), StrokeKind::Inside);
}

fn draw_point(painter: &egui::Painter, c: Pos2, r: f32, kind: SegmentKind, fill: Color32) {
    let stroke = Stroke::new(1.0_f32, WELL);
    match kind {
        SegmentKind::Curve => {
            painter.circle(c, r, fill, stroke);
        }
        SegmentKind::SCurve => {
            let r = r * 1.25;
            painter.add(Shape::convex_polygon(
                vec![c + vec2(0.0, -r), c + vec2(r, 0.0), c + vec2(0.0, r), c + vec2(-r, 0.0)],
                fill,
                stroke,
            ));
        }
        SegmentKind::Hold => {
            painter.rect(Rect::from_center_size(c, vec2(r * 1.8, r * 1.8)), 1.0, fill, stroke, StrokeKind::Inside);
        }
    }
}

/// Insert a point at (x, y) in sorted position; the new segment inherits the shape of the
/// segment it splits. Returns its index, or `None` when the pattern is full.
fn add_point(params: &GrainwaveParams, idx: usize, x: f32, y: f32) -> Option<usize> {
    let mut out = None;
    edit_pattern(params, idx, |pat| {
        if pat.points.len() >= MAX_POINTS {
            return;
        }
        let pos = pat.points.iter().position(|p| p.x > x).unwrap_or(pat.points.len());
        let kind = if pos > 0 {
            pat.points[pos - 1].kind
        } else {
            pat.points.last().map_or(SegmentKind::Curve, |p| p.kind)
        };
        pat.points.insert(pos, Point { x, y, tension: 0.0, kind });
        out = Some(pos);
    });
    out
}

/// Move point `i` to (x, y), keeping x strictly between its neighbours so indices stay stable.
pub fn move_point(points: &mut [Point], i: usize, x: f32, y: f32) {
    let n = points.len();
    if i >= n {
        return;
    }
    let lo = if i > 0 { points[i - 1].x + 0.001 } else { 0.0 };
    let hi = if i + 1 < n { points[i + 1].x - 0.001 } else { 1.0 };
    points[i].x = x.max(lo).min(hi.max(lo)).clamp(0.0, 1.0);
    points[i].y = y.clamp(0.0, 1.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_point_stays_between_neighbours() {
        let mut pts = vec![Point::new(0.0, 0.0), Point::new(0.5, 0.2), Point::new(0.75, 0.0)];
        move_point(&mut pts, 1, 0.9, 0.5);
        assert!(pts[1].x < 0.75 && pts[1].x > 0.7);
        move_point(&mut pts, 1, -1.0, 2.0);
        assert!(pts[1].x > 0.0 && pts[1].y == 1.0);
        move_point(&mut pts, 2, 2.0, 0.0);
        assert_eq!(pts[2].x, 1.0);
        move_point(&mut pts, 7, 0.5, 0.5); // out of range: ignored
    }

    #[test]
    fn edits_bump_version_and_add_points_in_order() {
        let p = GrainwaveParams::default();
        let v0 = p.pattern_version.load(Ordering::Relaxed);
        let i = add_point(&p, 3, 0.4, 0.25).unwrap();
        assert!(p.pattern_version.load(Ordering::Relaxed) > v0);
        let bank = read_bank(&p);
        let pts = &bank.patterns[3].points;
        assert_eq!(pts[i].x, 0.4);
        assert!(pts.windows(2).all(|w| w[0].x <= w[1].x));
    }

    #[test]
    fn snap_labels_include_triplets() {
        assert!(Snap::ALL.iter().any(|s| s.divisions() == Some(12.0)));
    }
}
