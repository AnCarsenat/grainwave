//! Time-shaper patterns: 12 editable curves, persisted with the plugin state.
//!
//! A pattern is a curve over one cycle: x in 0..1 (position in the cycle), y in 0..1.
//! The time shaper reads audio `y × depth × cycle_length` behind live:
//! - y = 0 everywhere: live, untouched audio.
//! - y rising with slope 1 (y = x): playback stands still (hold / repeat of the cycle start).
//! - y rising with slope 0.5: half speed. Slope > 1: plays backwards.
//! - a drop in y: jumps forward toward live; a rise in steps: stutter repeats.
//!
//! Threading: the GUI edits through `GrainwaveParams::time_patterns` (an `RwLock`), calls
//! `Pattern::sanitize`, then bumps `pattern_version`. The audio thread copies the bank with
//! `try_read` when the version changes (into preallocated storage) and never blocks.
//!
//! OWNERSHIP: data model + `value_at` = shared (coordinator). `factory_bank()` = feat/timeshape.

use serde::{Deserialize, Serialize};

pub const NUM_PATTERNS: usize = 12;
pub const MAX_POINTS: usize = 64;

/// Shape of the segment that starts at a point and runs to the next point.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SegmentKind {
    /// Power curve bent by tension (0 = straight line).
    #[default]
    Curve,
    /// Smooth S-curve (ease in/out), bent by tension.
    SCurve,
    /// Holds this point's y until the next point, then jumps.
    Hold,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
    /// -1..1. Positive = slow start, negative = fast start.
    #[serde(default)]
    pub tension: f32,
    #[serde(default)]
    pub kind: SegmentKind,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y, tension: 0.0, kind: SegmentKind::Curve }
    }
    pub const fn hold(x: f32, y: f32) -> Self {
        Self { x, y, tension: 0.0, kind: SegmentKind::Hold }
    }
    pub const fn curve(x: f32, y: f32, tension: f32) -> Self {
        Self { x, y, tension, kind: SegmentKind::Curve }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Pattern {
    pub name: String,
    /// Sorted by x, 1..=MAX_POINTS points, x and y in 0..1. Before the first point the curve
    /// equals the first point's y; after the last point it continues to the cycle end and
    /// wraps (segment from last point to first point + 1.0).
    pub points: Vec<Point>,
}

impl Pattern {
    pub fn flat(name: &str) -> Self {
        Self { name: name.to_string(), points: vec![Point::new(0.0, 0.0)] }
    }

    /// Clamp, sort and cap so `value_at` can trust the data. Call after every edit.
    pub fn sanitize(&mut self) {
        for p in &mut self.points {
            p.x = if p.x.is_finite() { p.x.clamp(0.0, 1.0) } else { 0.0 };
            p.y = if p.y.is_finite() { p.y.clamp(0.0, 1.0) } else { 0.0 };
            p.tension = if p.tension.is_finite() { p.tension.clamp(-1.0, 1.0) } else { 0.0 };
        }
        self.points.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
        self.points.truncate(MAX_POINTS);
        if self.points.is_empty() {
            self.points.push(Point::new(0.0, 0.0));
        }
    }

    /// Curve value at `x` (wrapped to 0..1). `global_tension` (-1..1) is added to each
    /// segment's tension. Allocation-free; safe on the audio thread.
    pub fn value_at(&self, x: f32, global_tension: f32) -> f32 {
        value_at(&self.points, x, global_tension)
    }
}

/// Slice version of `Pattern::value_at`, for preallocated audio-thread copies.
pub fn value_at(points: &[Point], x: f32, global_tension: f32) -> f32 {
    let n = points.len();
    if n == 0 {
        return 0.0;
    }
    if n == 1 {
        return points[0].y;
    }
    let x = if x.is_finite() { x - x.floor() } else { 0.0 };

    // Find segment start: last point with p.x <= x. Before the first point, the active
    // segment is the wrap-around one from the last point.
    let (a, ax, b, bx) = match points.iter().rposition(|p| p.x <= x) {
        Some(i) if i + 1 < n => (points[i], points[i].x, points[i + 1], points[i + 1].x),
        Some(i) => (points[i], points[i].x, points[0], points[0].x + 1.0),
        None => (points[n - 1], points[n - 1].x - 1.0, points[0], points[0].x),
    };
    let span = bx - ax;
    if span <= 1e-9 {
        return b.y;
    }
    let t = ((x - ax) / span).clamp(0.0, 1.0);
    let k = (a.tension + global_tension).clamp(-1.0, 1.0);
    let shaped = match a.kind {
        SegmentKind::Hold => return a.y,
        SegmentKind::Curve => bend(t, k),
        SegmentKind::SCurve => {
            let s = t * t * (3.0 - 2.0 * t);
            bend(s, k)
        }
    };
    a.y + (b.y - a.y) * shaped
}

/// Tension curve: k = 0 linear, k > 0 slow start (up to t^8), k < 0 fast start.
#[inline]
fn bend(t: f32, k: f32) -> f32 {
    if k.abs() < 1e-4 {
        t
    } else {
        t.powf((3.0 * k * std::f32::consts::LN_2).exp())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PatternBank {
    /// Exactly NUM_PATTERNS entries (index 0 = pattern 1).
    pub patterns: Vec<Pattern>,
}

impl PatternBank {
    pub fn sanitize(&mut self) {
        self.patterns.truncate(NUM_PATTERNS);
        while self.patterns.len() < NUM_PATTERNS {
            let i = self.patterns.len() + 1;
            self.patterns.push(Pattern::flat(&format!("Pattern {i}")));
        }
        for p in &mut self.patterns {
            p.sanitize();
        }
    }
}

impl Default for PatternBank {
    fn default() -> Self {
        let mut bank = factory_bank();
        bank.sanitize();
        bank
    }
}

/// OWNER: feat/timeshape. The 12 built-in patterns. Pattern 1 must stay flat (y = 0,
/// untouched audio) so a fresh instance is transparent.
///
/// Designed for sync 1/1 (one bar per cycle; at 1/2 everything happens twice as often).
/// Reminder of the semantics: a constant y plays the audio `y` cycles late (a hold step
/// after a live section = a repeat), slope 0.5 = half speed, slope 2 = reverse.
pub fn factory_bank() -> PatternBank {
    let named = |name: &str, points: Vec<Point>| Pattern { name: name.to_string(), points };
    // Hold steps: (x, y) pairs, each holding y until the next one.
    let steps = |name: &str, s: &[(f32, f32)]| {
        named(name, s.iter().map(|&(x, y)| Point::hold(x, y)).collect())
    };
    let scurve = |x: f32, y: f32| Point { x, y, tension: 0.0, kind: SegmentKind::SCurve };

    // Stutter roll: from x = 0.5 every step replays the slice starting at 0.5,
    // slices getting shorter: 2 x 1/8, 2 x 1/16, 4 x 1/32.
    let roll_starts = [0.5, 0.625, 0.75, 0.8125, 0.875, 0.906_25, 0.9375, 0.968_75];
    let mut roll = vec![(0.0, 0.0)];
    roll.extend(roll_starts.iter().map(|&x| (x, x - 0.5)));

    // Stairs 1/16: in each half bar, 4 live 16ths then 4 repeats of the next 16th.
    let mut stairs = Vec::new();
    for half in 0..2 {
        let base = half as f32 * 0.5;
        stairs.push((base, 0.0));
        for k in 0..4 {
            stairs.push((base + 0.25 + k as f32 / 16.0, k as f32 / 16.0));
        }
    }

    // Swing drag: per beat, the first 60% of the beat plays its first half slowly, the
    // rest catches up (delay peaks at 0.1 beat = 0.025 cycle).
    let mut swing = Vec::new();
    for b in 0..4 {
        let x = b as f32 * 0.25;
        swing.push(Point::new(x, 0.0));
        swing.push(Point::new(x + 0.25 * 0.6, 0.025));
    }

    let patterns = vec![
        Pattern::flat("Off"),
        // y = x / 2: the bar's first half stretched over the whole bar.
        named("Halftime", vec![Point::new(0.0, 0.0), Point::new(1.0, 0.5)]),
        // Last quarter: the 1/8 before it plays twice more.
        steps("Stutter 1/8 end", &[(0.0, 0.0), (0.75, 0.125), (0.875, 0.25)]),
        // Second half on the 12-step triplet grid: two triplet-8th rolls (A A A, B B B).
        steps(
            "Triplet stutter 1/12",
            &[
                (0.0, 0.0),
                (7.0 / 12.0, 1.0 / 12.0),
                (8.0 / 12.0, 2.0 / 12.0),
                (9.0 / 12.0, 0.0),
                (10.0 / 12.0, 1.0 / 12.0),
                (11.0 / 12.0, 2.0 / 12.0),
            ],
        ),
        steps("Stutter roll", &roll),
        // Beat 4 decelerates linearly to a stop (quadratic delay curve, tension 1/3 = t^2).
        named(
            "Tape stop",
            vec![Point::new(0.0, 0.0), Point::curve(0.75, 0.0, 1.0 / 3.0), Point::new(1.0, 0.125)],
        ),
        // Beat 4: delay rises with slope 2, so beat 3 plays backwards at normal speed.
        named(
            "Reverse beat 4",
            vec![Point::new(0.0, 0.0), Point::new(0.75, 0.0), Point::new(1.0, 0.5)],
        ),
        // Beats 1 1 3 3: each beat-long hold repeats the beat before it.
        steps("Repeat 1-1-3-3", &[(0.0, 0.0), (0.25, 0.25), (0.5, 0.0), (0.75, 0.25)]),
        steps("Stairs 1/16", &stairs),
        named("Swing drag", swing),
        // Beat 3 replays beat 1 (1 2 1 4).
        steps("Beat 1 on 3", &[(0.0, 0.0), (0.5, 0.5), (0.75, 0.0)]),
        // Two gentle S-curve swells per bar: about +-5% speed, a soft tape wobble.
        named(
            "Wobble",
            vec![scurve(0.0, 0.0), scurve(0.25, 0.008), scurve(0.5, 0.0), scurve(0.75, 0.008)],
        ),
    ];
    debug_assert_eq!(patterns.len(), NUM_PATTERNS);
    PatternBank { patterns }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_is_zero() {
        let p = Pattern::flat("x");
        for i in 0..100 {
            assert_eq!(p.value_at(i as f32 / 100.0, 0.0), 0.0);
        }
    }

    #[test]
    fn linear_ramp() {
        let p = Pattern { name: "r".into(), points: vec![Point::new(0.0, 0.0), Point::new(1.0, 1.0)] };
        assert!((p.value_at(0.25, 0.0) - 0.25).abs() < 1e-6);
        assert!((p.value_at(0.75, 0.0) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn hold_steps() {
        let p = Pattern {
            name: "h".into(),
            points: vec![Point::hold(0.0, 0.0), Point::hold(0.5, 0.5)],
        };
        assert_eq!(p.value_at(0.25, 0.0), 0.0);
        assert_eq!(p.value_at(0.75, 0.0), 0.5);
    }

    #[test]
    fn wraps_before_first_point() {
        let p = Pattern {
            name: "w".into(),
            points: vec![Point::new(0.5, 1.0), Point::new(0.75, 0.0)],
        };
        // From 0.75 (y=0) wrapping to 1.5 (y=1): at x=0.25 (=1.25) halfway.
        assert!((p.value_at(0.25, 0.0) - 0.666_666_7).abs() < 1e-4);
    }

    #[test]
    fn bank_has_twelve_and_first_is_flat() {
        let b = PatternBank::default();
        assert_eq!(b.patterns.len(), NUM_PATTERNS);
        for i in 0..50 {
            assert_eq!(b.patterns[0].value_at(i as f32 / 50.0, 0.0), 0.0);
        }
    }

    #[test]
    fn factory_patterns_finite_and_in_range() {
        let bank = factory_bank();
        assert_eq!(bank.patterns.len(), NUM_PATTERNS);
        assert_eq!(bank.patterns[0].name, "Off");
        for p in &bank.patterns {
            assert!(!p.points.is_empty() && p.points.len() <= MAX_POINTS, "{}", p.name);
            // Already sanitized: sanitize must not change anything.
            let mut q = p.clone();
            q.sanitize();
            assert_eq!(&q, p, "{} not sanitized", p.name);
            for tension in [-1.0, 0.0, 1.0] {
                for i in 0..1000 {
                    let y = p.value_at(i as f32 / 1000.0, tension);
                    assert!(y.is_finite() && (0.0..=1.0).contains(&y), "{} at {i}: {y}", p.name);
                }
            }
        }
        let names: std::collections::HashSet<_> = bank.patterns.iter().map(|p| &p.name).collect();
        assert_eq!(names.len(), NUM_PATTERNS, "names must be unique");
    }

    #[test]
    fn sanitize_fixes_garbage() {
        let mut p = Pattern {
            name: "g".into(),
            points: vec![Point::new(f32::NAN, 2.0), Point::new(0.5, -1.0)],
        };
        p.sanitize();
        for q in &p.points {
            assert!((0.0..=1.0).contains(&q.x) && (0.0..=1.0).contains(&q.y));
        }
    }
}
