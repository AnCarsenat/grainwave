//! Offscreen screenshots of the editor for the README.
//!
//! Renders the real editor with egui, tessellates it, and rasterizes the triangles on the CPU
//! (no GPU, no window, nothing from the desktop). Run with:
//!
//! ```sh
//! cargo test --release --lib screenshots -- --ignored
//! ```
//!
//! Writes `docs/screenshots/*.png`. Knob values are parameter defaults (nih-plug keeps
//! parameter setters crate-private); curves, sequencer lanes and live visuals are filled with
//! demo data through their public state.

use super::*;
use crate::pattern::factory_bank;
use crate::shared::store_f32;
use nih_plug_egui::egui::{
    epaint::{ClippedPrimitive, Primitive},
    Color32, Context, ImageData, Pos2, RawInput, TextureId,
};
use std::collections::HashMap;
use std::sync::atomic::Ordering;

const PPP: f32 = 1.5;

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

/// RGBA8 texture, premultiplied.
struct Texture {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
}

impl Texture {
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let x = (u * self.w as f32 - 0.5).clamp(0.0, self.w as f32 - 1.0);
        let y = (v * self.h as f32 - 0.5).clamp(0.0, self.h as f32 - 1.0);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let p = |x: usize, y: usize| self.px[y * self.w + x];
        let mut out = [0.0; 4];
        for (c, o) in out.iter_mut().enumerate() {
            let top = p(x0, y0)[c] * (1.0 - fx) + p(x1, y0)[c] * fx;
            let bot = p(x0, y1)[c] * (1.0 - fx) + p(x1, y1)[c] * fx;
            *o = top * (1.0 - fy) + bot * fy;
        }
        out
    }
}

fn to_f(c: Color32) -> [f32; 4] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, c.a() as f32 / 255.0]
}

fn apply_delta(textures: &mut HashMap<TextureId, Texture>, delta: &egui::TexturesDelta) {
    for (id, d) in &delta.set {
        let (w, h, px): (usize, usize, Vec<[f32; 4]>) = match &d.image {
            ImageData::Color(img) => (img.size[0], img.size[1], img.pixels.iter().map(|c| to_f(*c)).collect()),
            ImageData::Font(img) => (img.size[0], img.size[1], img.srgba_pixels(None).map(to_f).collect()),
        };
        match d.pos {
            None => {
                textures.insert(*id, Texture { w, h, px });
            }
            Some([ox, oy]) => {
                let t = textures.get_mut(id).expect("partial update of unknown texture");
                for y in 0..h {
                    for x in 0..w {
                        t.px[(oy + y) * t.w + ox + x] = px[y * w + x];
                    }
                }
            }
        }
    }
}

/// Premultiplied "over" blend, gamma space (matches egui's default renderer).
fn rasterize(prims: &[ClippedPrimitive], textures: &HashMap<TextureId, Texture>, w: usize, h: usize, bg: Color32) -> Vec<u8> {
    let mut fb = vec![to_f(bg); w * h];
    for prim in prims {
        let Primitive::Mesh(mesh) = &prim.primitive else { continue };
        let Some(tex) = textures.get(&mesh.texture_id) else { continue };
        let clip = prim.clip_rect;
        let (cx0, cy0) = (((clip.min.x * PPP).max(0.0)) as usize, ((clip.min.y * PPP).max(0.0)) as usize);
        let (cx1, cy1) = (((clip.max.x * PPP).ceil() as usize).min(w), ((clip.max.y * PPP).ceil() as usize).min(h));
        for tri in mesh.indices.chunks_exact(3) {
            let v = [&mesh.vertices[tri[0] as usize], &mesh.vertices[tri[1] as usize], &mesh.vertices[tri[2] as usize]];
            let p: Vec<[f32; 2]> = v.iter().map(|v| [v.pos.x * PPP, v.pos.y * PPP]).collect();
            let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1]);
            if area.abs() < 1e-6 {
                continue;
            }
            let minx = p.iter().map(|q| q[0]).fold(f32::MAX, f32::min).floor().max(cx0 as f32) as usize;
            let maxx = (p.iter().map(|q| q[0]).fold(f32::MIN, f32::max).ceil() as usize).min(cx1);
            let miny = p.iter().map(|q| q[1]).fold(f32::MAX, f32::min).floor().max(cy0 as f32) as usize;
            let maxy = (p.iter().map(|q| q[1]).fold(f32::MIN, f32::max).ceil() as usize).min(cy1);
            let cols = [to_f(v[0].color), to_f(v[1].color), to_f(v[2].color)];
            for y in miny..maxy {
                for x in minx..maxx {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = ((p[1][0] - px) * (p[2][1] - py) - (p[2][0] - px) * (p[1][1] - py)) / area;
                    let w1 = ((p[2][0] - px) * (p[0][1] - py) - (p[0][0] - px) * (p[2][1] - py)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                        continue;
                    }
                    let u = w0 * v[0].uv.x + w1 * v[1].uv.x + w2 * v[2].uv.x;
                    let vv = w0 * v[0].uv.y + w1 * v[1].uv.y + w2 * v[2].uv.y;
                    let t = tex.sample(u, vv);
                    let mut src = [0.0; 4];
                    for c in 0..4 {
                        src[c] = (w0 * cols[0][c] + w1 * cols[1][c] + w2 * cols[2][c]) * t[c];
                    }
                    let d = &mut fb[y * w + x];
                    for c in 0..4 {
                        d[c] = src[c] + d[c] * (1.0 - src[3]);
                    }
                }
            }
        }
    }
    fb.iter()
        .flat_map(|c| [c[0], c[1], c[2], 1.0].map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8))
        .collect()
}

fn demo_visuals(visual: &VisualState) {
    for (i, a) in visual.waveform.iter().enumerate() {
        let x = i as f32 / visual.waveform.len() as f32;
        let beat = (x * 8.0).fract();
        let env = (-beat * 5.0).exp() * 0.8 + 0.12;
        let tex = 0.75 + 0.25 * (x * 97.0).sin() * (x * 31.0).cos();
        store_f32(a, (env * tex).clamp(0.0, 1.0));
    }
    store_f32(&visual.write_head, 0.62);
    let positions = [0.08, 0.17, 0.24, 0.33, 0.41, 0.47, 0.52, 0.56, 0.6];
    let mut active = 0u32;
    for (i, p) in positions.iter().enumerate() {
        store_f32(&visual.grain_pos[i], *p);
        active |= 1 << i;
    }
    visual.grain_active.store(active, Ordering::Relaxed);
    visual.grain_reverse.store(0b0_1001_0010, Ordering::Relaxed);
    store_f32(&visual.lfo_values[0], 0.55);
    store_f32(&visual.lfo_values[1], -0.3);
    store_f32(&visual.env_value, 0.62);
    store_f32(&visual.ts_cursor, 0.66);
    store_f32(&visual.ts_value, 0.25);
    visual.ts_active.store(0, Ordering::Relaxed);
    visual.gate_step.store(5, Ordering::Relaxed);
    visual.seq_step.store(5, Ordering::Relaxed);
    // [time, grains, rhythm, degrade, filter, space]: grains at its default amount of 0.
    for (a, g) in visual.section_gain.iter().zip([1.0, 0.0, 1.0, 1.0, 1.0, 1.0]) {
        store_f32(a, g);
    }
}

fn demo_state(params: &GrainwaveParams) {
    // Show a stutter curve in the viewed slot (slots are user-editable).
    {
        let mut bank = params.time_patterns.write().unwrap();
        let factory = factory_bank();
        bank.patterns[0] = factory.patterns[3].clone();
        bank.sanitize();
    }
    params.pattern_version.fetch_add(1, Ordering::Relaxed);
    let lanes = params.seq_lane_atomics();
    use crate::params::SEQ_LANE_ACTIVE;
    lanes[3].store(SEQ_LANE_ACTIVE | (1 << 3) | (1 << 7) | (1 << 11) | (1 << 15), Ordering::Relaxed);
    lanes[5].store(SEQ_LANE_ACTIVE | (1 << 4) | (1 << 5) | (1 << 12) | (1 << 13), Ordering::Relaxed);
    lanes[0].store(SEQ_LANE_ACTIVE | 0xF0F0, Ordering::Relaxed);
}

fn shoot(name: &str, w: f32, h: f32, state: &mut EditorState, params: &GrainwaveParams, visual: &VisualState) {
    let ctx = Context::default();
    apply_theme(&ctx);
    ctx.set_pixels_per_point(PPP);
    let gui = NullContext;
    let setter = ParamSetter::new(&gui);
    let mut textures = HashMap::new();
    let mut last = None;
    for _ in 0..4 {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(w, h))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                draw(ui, params, visual, &setter, state);
            });
        });
        apply_delta(&mut textures, &out.textures_delta);
        last = Some(out);
    }
    let out = last.unwrap();
    let prims = ctx.tessellate(out.shapes, PPP);
    let (pw, ph) = ((w * PPP) as usize, (h * PPP) as usize);
    let bg = ctx.style().visuals.panel_fill;
    let rgba = rasterize(&prims, &textures, pw, ph, bg);

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/screenshots");
    std::fs::create_dir_all(&dir).unwrap();
    let file = std::fs::File::create(dir.join(format!("{name}.png"))).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), pw as u32, ph as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Best);
    enc.write_header().unwrap().write_image_data(&rgba).unwrap();
}

#[test]
#[ignore = "writes docs/screenshots; run manually"]
fn screenshots() {
    let params = GrainwaveParams::default();
    let visual = VisualState::default();
    demo_state(&params);
    demo_visuals(&visual);

    let mut state = EditorState::default();
    shoot("simple", 960.0, 640.0, &mut state, &params, &visual);
    state.page = Page::Advanced;
    for (tab, name) in [
        (Tab::Time, "advanced-time"),
        (Tab::Grains, "advanced-grains"),
        (Tab::Degrade, "advanced-degrade"),
        (Tab::Sequencer, "advanced-sequencer"),
    ] {
        state.tab = tab;
        shoot(name, 960.0, 640.0, &mut state, &params, &visual);
    }
}
