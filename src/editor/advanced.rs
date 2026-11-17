//! Advanced page: one tab per section with every parameter, each tab header carrying the
//! section's power switch, dry/wet fader and randomize locks.
//!
//! Height budget: tab bar `TAB_H` + 4, header `HEADER_H` + 6, the rest is the tab's content
//! (default window: 572 − 54 = 518 px, minimum: 438 px). Every grid inside uses `Cells`, which
//! shrinks cells instead of overflowing, so no tab needs a scroll area.

use super::curve::curve_panel;
use super::randomize::Group;
use super::sequencer::sequencer_grid;
use super::simple::{seq_header, tab_button};
use super::theme::*;
use super::widgets::{boxes, group_box, meter, momentary, power, slider, toggle, Cells};
use super::EditorState;
use crate::params::GrainwaveParams;
use crate::shared::{load_f32, VisualState, MAX_VISUAL_GRAINS, WAVEFORM_POINTS};
use nih_plug::prelude::*;
use nih_plug_egui::egui::{pos2, vec2, Align2, Color32, FontId, Id, Rect, Sense, Shape, Stroke, StrokeKind, Ui};
use std::sync::atomic::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Time,
    Grains,
    Rhythm,
    Degrade,
    Filter,
    Space,
    Modulation,
    Sequencer,
}

impl Tab {
    pub const ALL: [Tab; 8] = [
        Tab::Time,
        Tab::Grains,
        Tab::Rhythm,
        Tab::Degrade,
        Tab::Filter,
        Tab::Space,
        Tab::Modulation,
        Tab::Sequencer,
    ];

    fn label(self) -> &'static str {
        match self {
            Tab::Time => "Time",
            Tab::Grains => "Grains",
            Tab::Rhythm => "Rhythm",
            Tab::Degrade => "Degrade",
            Tab::Filter => "Filter",
            Tab::Space => "Space",
            Tab::Modulation => "Modulation",
            Tab::Sequencer => "Sequencer",
        }
    }

    fn color(self) -> Color32 {
        match self {
            Tab::Time => C_TIME,
            Tab::Grains => C_GRAINS,
            Tab::Rhythm => C_RHYTHM,
            Tab::Degrade => C_DEGRADE,
            Tab::Filter => C_FILTER,
            Tab::Space => C_SPACE,
            Tab::Modulation => C_MOD,
            Tab::Sequencer => TEXT,
        }
    }

    fn groups(self) -> &'static [Group] {
        match self {
            Tab::Time => &[Group::Time],
            Tab::Grains => &[Group::Input, Group::Grains, Group::Playback],
            Tab::Rhythm => &[Group::Rhythm],
            Tab::Degrade => &[Group::Degrade],
            Tab::Filter => &[Group::Filter],
            Tab::Space => &[Group::Space],
            Tab::Modulation => &[Group::Modulation, Group::Macros],
            Tab::Sequencer => &[],
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Tab::Time => "Drawn curve per cycle = how far behind live playback reads",
            Tab::Grains => "Amount 0 = grains silent. Turn it up to blend the cloud in",
            Tab::Rhythm => "Random stutters, swing, 16-step gate and hold buttons",
            Tab::Degrade => "Bit crush, tape wobble, saturation, noise and dropouts",
            Tab::Filter => "Resonant low/high-pass with drift",
            Tab::Space => "Reverb and synced delay",
            Tab::Modulation => "Two LFOs, an envelope follower and the macros",
            Tab::Sequencer => "Switch each section on or off per step",
        }
    }
}

pub fn advanced_page(ui: &mut Ui, rect: Rect, p: &GrainwaveParams, visual: &VisualState, s: &ParamSetter, state: &mut EditorState) {
    // Tab bar.
    let n = Tab::ALL.len() as f32;
    let tw = ((rect.width() - 3.0 * (n - 1.0)) / n).min(110.0);
    for (i, tab) in Tab::ALL.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + i as f32 * (tw + 3.0), rect.top()), vec2(tw, TAB_H));
        if tab_button(ui, r, tab.label(), state.tab == *tab, tab.color()) {
            state.tab = *tab;
        }
    }
    let body = Rect::from_min_max(pos2(rect.left(), rect.top() + TAB_H + 4.0), rect.max);
    let painter = ui.painter_at(body);
    painter.rect_filled(body, 5.0, PANEL);
    painter.rect_stroke(body, 5.0, Stroke::new(1.0_f32, PANEL_EDGE), StrokeKind::Inside);
    let inner = body.shrink(PAD);
    let header = Rect::from_min_size(inner.min, vec2(inner.width(), HEADER_H));
    tab_header(ui, header, p, s, state);
    let content = Rect::from_min_max(pos2(inner.left(), header.bottom() + 6.0), inner.max);

    match state.tab {
        Tab::Time => time_tab(ui, content, p, visual, s, state),
        Tab::Grains => grains_tab(ui, content, p, visual, s),
        Tab::Rhythm => rhythm_tab(ui, content, p, visual, s, state),
        Tab::Degrade => degrade_tab(ui, content, p, s),
        Tab::Filter => {
            let mut c = Cells::new(C_FILTER);
            let f = &p.filter;
            c.knob(s, &f.lowpass_hz, "Low-pass")
                .knob(s, &f.lowpass_res, "LP res")
                .knob(s, &f.highpass_hz, "High-pass")
                .knob(s, &f.highpass_res, "HP res")
                .knob(s, &f.drift, "Drift");
            let half = Rect::from_min_size(content.min, vec2(content.width(), content.height().min(160.0)));
            boxes(ui, half, vec![("FILTER", C_FILTER, c)]);
        }
        Tab::Space => {
            let sp = &p.space;
            let mut rev = Cells::new(C_SPACE);
            rev.knob(s, &sp.reverb_size, "Size").knob(s, &sp.reverb_mix, "Mix");
            let mut dly = Cells::new(C_SPACE);
            dly.combo(s, &sp.delay_division, "Time")
                .knob(s, &sp.delay_feedback, "Feedback")
                .knob(s, &sp.delay_mix, "Mix")
                .knob(s, &sp.delay_crush, "Crush");
            let half = Rect::from_min_size(content.min, vec2(content.width(), content.height().min(160.0)));
            boxes(ui, half, vec![("REVERB", C_SPACE, rev), ("DELAY", C_SPACE, dly)]);
        }
        Tab::Modulation => modulation_tab(ui, content, p, visual, s),
        Tab::Sequencer => {
            let head = Rect::from_min_size(content.min, vec2(content.width(), HEADER_H));
            seq_header(ui, head, p, visual, s);
            let grid_h = (content.height() - HEADER_H - 40.0).min(260.0);
            let grid = Rect::from_min_size(pos2(content.left(), head.bottom() + 8.0), vec2(content.width(), grid_h));
            sequencer_grid(ui, grid, p, visual, &mut state.seq);
            ui.painter_at(content).text(
                pos2(content.left(), grid.bottom() + 10.0),
                Align2::LEFT_TOP,
                "Click a cell to toggle, drag to paint. LED = lane active (inactive lanes never gate). \
                 Right-click a lane name for quick patterns.",
                FontId::proportional(10.5),
                TEXT_DIM,
            );
        }
    }
}

/// Power switch, dry/wet fader, hint and randomize locks for the current tab.
fn tab_header(ui: &mut Ui, r: Rect, p: &GrainwaveParams, s: &ParamSetter, state: &mut EditorState) {
    let tab = state.tab;
    let color = tab.color();
    let sec = &p.sections;
    let (on, name, mix): (&BoolParam, &str, Option<&FloatParam>) = match tab {
        Tab::Time => (&sec.time, "TIME SHAPER", Some(&p.time.mix)),
        Tab::Grains => (&sec.grains, "GRAINS", Some(&sec.grain_amount)),
        Tab::Rhythm => (&sec.rhythm, "RHYTHM", Some(&sec.rhythm_mix)),
        Tab::Degrade => (&sec.degrade, "DEGRADE", Some(&sec.degrade_mix)),
        Tab::Filter => (&sec.filter, "FILTER", Some(&sec.filter_mix)),
        Tab::Space => (&sec.space, "SPACE", Some(&sec.space_mix)),
        Tab::Modulation => (&sec.modulation, "MODULATION", None),
        Tab::Sequencer => (&p.sequencer.enabled, "FX SEQUENCER", None),
    };
    let pw = Rect::from_min_size(r.min, vec2(130.0, r.height()));
    power(ui, pw, on, s, name, color);
    let mut x = pw.right() + 10.0;
    let painter = ui.painter_at(r);
    if let Some(mix) = mix {
        let label = if tab == Tab::Grains { "Amount" } else { "Dry/wet" };
        painter.text(pos2(x, r.center().y), Align2::LEFT_CENTER, label, FontId::proportional(10.5), TEXT_DIM);
        let fr = Rect::from_min_size(pos2(x + 48.0, r.top() + 4.0), vec2(150.0, r.height() - 8.0));
        slider(ui, fr, mix, s, color, false);
        painter.text(pos2(fr.right() + 6.0, r.center().y), Align2::LEFT_CENTER, mix.to_string(), FontId::proportional(10.5), TEXT);
        x = fr.right() + 46.0;
    }
    // Locks, right-aligned.
    let mut lx = r.right();
    for g in tab.groups().iter().rev() {
        let locked = state.locks[*g as usize];
        let text = if locked { format!("{} locked", g.label()) } else { format!("lock {}", g.label()) };
        let w = 12.0 + 6.0 * text.len() as f32;
        let lr = Rect::from_min_size(pos2(lx - w, r.top() + 1.0), vec2(w, r.height() - 2.0));
        let resp = ui.interact(lr, Id::new(("lock", *g as usize)), Sense::click());
        if resp.clicked() {
            state.locks[*g as usize] = !locked;
        }
        let fill = if locked { dim(HOT, 0.3) } else if resp.hovered() { Color32::from_rgb(0x3a, 0x33, 0x2d) } else { Color32::from_rgb(0x2c, 0x27, 0x23) };
        painter.rect_filled(lr, 3.0, fill);
        painter.text(lr.center(), Align2::CENTER_CENTER, text, FontId::proportional(10.0), if locked { HOT } else { TEXT_DIM });
        resp.on_hover_text("Locked groups are skipped by Randomize");
        lx = lr.left() - 4.0;
    }
    if lx - x > 120.0 {
        let hint_rect = Rect::from_min_max(pos2(x, r.top()), pos2(lx - 4.0, r.bottom()));
        ui.painter_at(hint_rect).text(pos2(x, r.center().y), Align2::LEFT_CENTER, tab.hint(), FontId::proportional(10.0), TEXT_DIM);
    }
}

fn time_tab(ui: &mut Ui, rect: Rect, p: &GrainwaveParams, visual: &VisualState, s: &ParamSetter, state: &mut EditorState) {
    let side_w = (rect.width() * 0.3).clamp(220.0, 300.0);
    let editor = Rect::from_min_max(rect.min, pos2(rect.right() - side_w - GAP, rect.bottom()));
    let side = Rect::from_min_max(pos2(rect.right() - side_w, rect.top()), rect.max);
    curve_panel(ui, editor, 0.0, p, visual, s, &mut state.curve);
    let t = &p.time;
    let mut c = Cells::new(C_TIME);
    c.knob(s, &t.pattern, "Pattern")
        .combo(s, &t.sync, "Sync")
        .knob(s, &t.depth, "Depth")
        .combo(s, &t.trigger, "Trigger")
        .toggle(s, &t.midi_switch, "MIDI sw")
        .combo(s, &t.pattern_quantize, "Switch on")
        .knob(s, &t.phase, "Phase")
        .knob_bi(s, &t.tension, "Tension")
        .knob(s, &t.smooth, "Smooth")
        .combo(s, &t.anti_click, "Anti-click");
    let inner = group_box(ui, side, "TIME SHAPER", C_TIME);
    c.show(ui, inner);
}

fn grains_tab(ui: &mut Ui, rect: Rect, p: &GrainwaveParams, visual: &VisualState, s: &ParamSetter) {
    let wave_h = (rect.height() * 0.36).clamp(90.0, 200.0);
    let wave = Rect::from_min_size(rect.min, vec2(rect.width(), wave_h));
    waveform(ui, wave, p, visual, s);
    let rest = Rect::from_min_max(pos2(rect.left(), wave.bottom() + GAP), rect.max);

    let i = &p.input;
    let mut input = Cells::new(C_GRAINS);
    input
        .knob(s, &i.input_gain, "In gain")
        .combo(s, &i.channel_mode, "Channel")
        .combo(s, &i.buffer_mode, "Buffer")
        .knob(s, &i.buffer_bars, "Bars")
        .knob(s, &i.buffer_seconds, "Seconds");
    let g = &p.grain;
    let mut grains = Cells::new(C_GRAINS);
    grains
        .knob(s, &g.size_ms, "Size")
        .combo(s, &g.density_mode, "Density")
        .knob(s, &g.density_hz, "Rate")
        .combo(s, &g.density_division, "Division")
        .knob(s, &g.position, "Position")
        .knob(s, &g.position_jitter, "Jitter")
        .knob(s, &g.memory, "Memory")
        .knob(s, &g.spread, "Spread")
        .knob(s, &g.max_voices, "Voices")
        .combo(s, &g.envelope, "Envelope")
        .toggle(s, &g.low_cpu, "Low CPU");
    let pb = &p.playback;
    let mut play = Cells::new(C_GRAINS);
    play.knob(s, &pb.reverse_chance, "Reverse")
        .knob_bi(s, &pb.pitch_st, "Pitch")
        .knob(s, &pb.pitch_jitter_st, "P. jitter")
        .combo(s, &pb.pitch_quantize, "Quantize")
        .knob(s, &pb.speed, "Speed")
        .knob(s, &pb.tape_stop_chance, "Stop %")
        .knob(s, &pb.tape_start_chance, "Start %")
        .toggle(s, &pb.midi_pitch, "MIDI pitch");
    boxes(ui, rest, vec![("INPUT", C_GRAINS, input), ("GRAINS", C_GRAINS, grains), ("PLAYBACK", C_GRAINS, play)]);
}

fn rhythm_tab(ui: &mut Ui, rect: Rect, p: &GrainwaveParams, visual: &VisualState, s: &ParamSetter, state: &mut EditorState) {
    let r = &p.rhythm;
    let gate_h = 86.0;
    let top = Rect::from_min_max(rect.min, pos2(rect.right(), (rect.bottom() - gate_h - GAP).min(rect.top() + 170.0)));
    let gate = Rect::from_min_max(pos2(rect.left(), top.bottom() + GAP), pos2(rect.right(), top.bottom() + GAP + gate_h));

    let mut stut = Cells::new(C_RHYTHM);
    stut.combo(s, &r.stutter_division, "Stutter").knob(s, &r.stutter_chance, "Chance").knob(s, &r.swing, "Swing");
    let mut hold = Cells::new(C_RHYTHM);
    let (rw_held, rw_drawn) = (&mut state.rewind_held, &mut state.rewind_drawn);
    let (ts_held, ts_drawn) = (&mut state.tape_stop_held, &mut state.tape_stop_drawn);
    hold.knob(s, &r.tape_stop_time_ms, "Stop time")
        .add(2, move |ui, cell| {
            let b = Rect::from_center_size(cell.center(), vec2(cell.width() - 8.0, (cell.height() - 16.0).clamp(20.0, 44.0)));
            momentary(ui, b, &r.rewind, s, "Rewind", rw_held, rw_drawn);
        })
        .add(2, move |ui, cell| {
            let b = Rect::from_center_size(cell.center(), vec2(cell.width() - 8.0, (cell.height() - 16.0).clamp(20.0, 44.0)));
            momentary(ui, b, &r.tape_stop, s, "Tape stop", ts_held, ts_drawn);
        });
    boxes(ui, top, vec![("STUTTER & SWING", C_RHYTHM, stut), ("HOLD (press and hold)", C_RHYTHM, hold)]);

    // Gate: toggle + 16 steps.
    let inner = group_box(ui, gate, "GATE", C_RHYTHM);
    let tg = Rect::from_min_size(pos2(inner.left(), inner.center().y - 10.0), vec2(60.0, 20.0));
    toggle(ui, tg, &r.gate_enabled, s, "Gate", C_RHYTHM);
    let grid = Rect::from_min_max(pos2(tg.right() + 10.0, inner.top() + 4.0), pos2(inner.right(), inner.bottom() - 4.0));
    gate_grid(ui, grid, p, visual, s);
}

fn gate_grid(ui: &mut Ui, rect: Rect, p: &GrainwaveParams, visual: &VisualState, s: &ParamSetter) {
    let r = &p.rhythm;
    let gap = 3.0;
    let cell_w = ((rect.width() - 15.0 * gap) / 16.0).max(4.0);
    let current = visual.gate_step.load(Ordering::Relaxed) as usize % 16;
    let gate_on = r.gate_enabled.value();
    for (i, step) in r.gate_steps.iter().enumerate() {
        let x = rect.left() + i as f32 * (cell_w + gap);
        let cell = Rect::from_min_size(pos2(x, rect.top()), vec2(cell_w, rect.height()));
        let resp = ui.interact(cell, Id::new(("gate_step", i)), Sense::click());
        let on = step.on.value();
        if resp.clicked() {
            s.begin_set_parameter(&step.on);
            s.set_parameter(&step.on, !on);
            s.end_set_parameter(&step.on);
        }
        let fill = match (on, gate_on) {
            (true, true) => C_RHYTHM,
            (true, false) => dim(C_RHYTHM, 0.4),
            (false, _) => {
                if i % 4 == 0 {
                    CELL_OFF_BEAT
                } else {
                    CELL_OFF
                }
            }
        };
        let painter = ui.painter();
        painter.rect_filled(cell, 3.0, fill);
        if gate_on && i == current {
            painter.rect_stroke(cell.expand(1.0), 3.0, Stroke::new(2.0_f32, TEXT), StrokeKind::Outside);
        } else if resp.hovered() {
            painter.rect_stroke(cell, 3.0, Stroke::new(1.0_f32, TEXT_DIM), StrokeKind::Inside);
        }
        resp.on_hover_text(format!("Gate step {}", i + 1));
    }
}

fn degrade_tab(ui: &mut Ui, rect: Rect, p: &GrainwaveParams, s: &ParamSetter) {
    let d = &p.degrade;
    let row_h = (rect.height() - GAP) / 2.0;
    let row1 = Rect::from_min_size(rect.min, vec2(rect.width(), row_h));
    let row2 = Rect::from_min_size(pos2(rect.left(), row1.bottom() + GAP), vec2(rect.width(), row_h));
    let mut digital = Cells::new(C_DEGRADE);
    digital
        .combo(s, &d.order, "Order")
        .knob(s, &d.bit_depth, "Bits")
        .knob(s, &d.sample_rate_hz, "Rate")
        .combo(s, &d.oversampling, "Oversample");
    let mut tape = Cells::new(C_DEGRADE);
    tape.knob(s, &d.wow_rate_hz, "Wow Hz")
        .knob(s, &d.wow_depth, "Wow")
        .knob(s, &d.flutter_rate_hz, "Flut. Hz")
        .knob(s, &d.flutter_depth, "Flutter")
        .combo(s, &d.saturation_mode, "Saturation")
        .knob(s, &d.drive, "Drive")
        .knob(s, &d.tape_age, "Tape age")
        .knob(s, &d.head_bump_db, "Bump");
    boxes(ui, row1, vec![("DIGITAL", C_DEGRADE, digital), ("TAPE", C_DEGRADE, tape)]);
    let mut noise = Cells::new(C_DEGRADE);
    noise
        .knob(s, &d.hiss, "Hiss")
        .knob(s, &d.crackle_density, "Crackle")
        .knob(s, &d.crackle_level, "Crk lvl")
        .knob(s, &d.dropout_chance, "Dropout")
        .knob(s, &d.dropout_depth, "Drop dpt");
    let mut macros = Cells::new(C_DEGRADE);
    macros.knob(s, &p.macros.dust, "Dust").knob(s, &p.macros.age, "Age");
    boxes(ui, row2, vec![("NOISE & DROPOUTS", C_DEGRADE, noise), ("MACROS", C_DEGRADE, macros)]);
}

fn modulation_tab(ui: &mut Ui, rect: Rect, p: &GrainwaveParams, visual: &VisualState, s: &ParamSetter) {
    let m = &p.modulation;
    let row_h = (rect.height() - GAP) / 2.0;
    let row1 = Rect::from_min_size(rect.min, vec2(rect.width(), row_h));
    let row2 = Rect::from_min_size(pos2(rect.left(), row1.bottom() + GAP), vec2(rect.width(), row_h));
    let lfo_cells = |i: usize| {
        let lfo = &m.lfos[i];
        let mut c = Cells::new(C_MOD);
        c.add(1, move |ui, r| {
            let mr = Rect::from_center_size(r.center(), vec2(8.0, (r.height() - 12.0).max(8.0)));
            meter(&ui.painter_at(r), mr, load_f32(&visual.lfo_values[i]), true, C_MOD);
        })
        .combo(s, &lfo.shape, "Shape")
        .combo(s, &lfo.rate_mode, "Rate mode")
        .knob(s, &lfo.rate_hz, "Hz")
        .combo(s, &lfo.target, "Target")
        .knob_bi(s, &lfo.amount, "Amount");
        c
    };
    boxes(ui, row1, vec![("LFO 1", C_MOD, lfo_cells(0)), ("LFO 2", C_MOD, lfo_cells(1))]);
    let mut env = Cells::new(C_MOD);
    env.add(1, move |ui, r| {
        let mr = Rect::from_center_size(r.center(), vec2(8.0, (r.height() - 12.0).max(8.0)));
        meter(&ui.painter_at(r), mr, load_f32(&visual.env_value), false, C_MOD);
    })
    .combo(s, &m.env_target, "Target")
    .knob_bi(s, &m.env_amount, "Amount")
    .knob(s, &m.env_attack_ms, "Attack")
    .knob(s, &m.env_release_ms, "Release");
    let mut macros = Cells::new(C_MOD);
    macros.knob(s, &p.macros.dust, "Dust").knob(s, &p.macros.chaos, "Chaos").knob(s, &p.macros.age, "Age");
    boxes(ui, row2, vec![("ENVELOPE FOLLOWER", C_MOD, env), ("MACROS", C_MOD, macros)]);
}

/// Capture buffer with write head and grain markers, plus the freeze toggle.
fn waveform(ui: &mut Ui, rect: Rect, p: &GrainwaveParams, visual: &VisualState, s: &ParamSetter) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, WELL);
    painter.rect_stroke(rect, 4.0, Stroke::new(1.0_f32, PANEL_EDGE), StrokeKind::Inside);

    let frozen = p.input.freeze.value();
    let inner = rect.shrink(6.0);
    let mid = inner.center().y;
    let half = inner.height() * 0.5;
    for k in 1..8 {
        let x = inner.left() + inner.width() * k as f32 / 8.0;
        painter.line_segment([pos2(x, inner.top()), pos2(x, inner.bottom())], Stroke::new(1.0_f32, GRID_LINE));
    }
    painter.line_segment([pos2(inner.left(), mid), pos2(inner.right(), mid)], Stroke::new(1.0_f32, PANEL_EDGE));

    let wave_color = if frozen { Color32::from_rgb(0x7e, 0x93, 0x9a) } else { WAVE };
    let slice_w = inner.width() / WAVEFORM_POINTS as f32;
    for i in 0..WAVEFORM_POINTS {
        let peak = load_f32(&visual.waveform[i]);
        let a = if peak.is_finite() { peak.clamp(0.0, 1.0) } else { 0.0 };
        if a <= 0.0 {
            continue;
        }
        let x = inner.left() + (i as f32 + 0.5) * slice_w;
        let y = a.sqrt() * half;
        painter.line_segment([pos2(x, mid - y), pos2(x, mid + y)], Stroke::new(slice_w.max(1.0) * 0.8, wave_color));
    }

    let wh = load_f32(&visual.write_head);
    if wh.is_finite() {
        let x = inner.left() + wh.clamp(0.0, 1.0) * inner.width();
        let c = if frozen { TEXT_DIM } else { HOT };
        painter.line_segment([pos2(x, inner.top()), pos2(x, inner.bottom())], Stroke::new(1.5_f32, c));
    }

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
        let lane = (i % 8) as f32 / 7.0;
        let y = inner.top() + 8.0 + lane * (inner.height() - 16.0);
        let rev = reverse & (1 << i) != 0;
        let c = if rev { ACCENT_2 } else { ACCENT };
        painter.line_segment([pos2(x, y - 6.0), pos2(x, y + 6.0)], Stroke::new(1.0_f32, c.gamma_multiply(0.6)));
        if rev {
            painter.add(Shape::convex_polygon(
                vec![pos2(x - 4.0, y), pos2(x + 3.0, y - 4.0), pos2(x + 3.0, y + 4.0)],
                c,
                Stroke::NONE,
            ));
        } else {
            painter.circle_filled(pos2(x, y), 3.5, c);
        }
    }

    painter.text(pos2(inner.left() + 4.0, inner.top() + 2.0), Align2::LEFT_TOP, "buffer", FontId::proportional(10.0), TEXT_DIM);
    let legend_y = inner.bottom() - 2.0;
    painter.circle_filled(pos2(inner.left() + 8.0, legend_y - 5.0), 3.0, ACCENT);
    painter.text(pos2(inner.left() + 14.0, legend_y), Align2::LEFT_BOTTOM, "forward", FontId::proportional(10.0), TEXT_DIM);
    painter.circle_filled(pos2(inner.left() + 66.0, legend_y - 5.0), 3.0, ACCENT_2);
    painter.text(pos2(inner.left() + 72.0, legend_y), Align2::LEFT_BOTTOM, "reverse", FontId::proportional(10.0), TEXT_DIM);
    if p.sections.grain_amount.value() <= 0.0 {
        painter.text(
            pos2(inner.center().x, inner.top() + 2.0),
            Align2::CENTER_TOP,
            "grain amount is 0: grains are not heard",
            FontId::proportional(10.0),
            C_GRAINS,
        );
    }

    let btn = Rect::from_min_size(pos2(rect.right() - 78.0, rect.top() + 6.0), vec2(70.0, 20.0));
    toggle(ui, btn, &p.input.freeze, s, if frozen { "FROZEN" } else { "FREEZE" }, HOT);
}
