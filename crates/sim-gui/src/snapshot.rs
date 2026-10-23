//! Test helper: renders egui frames on the CPU into PNG files so the GUI can be looked at
//! without a display (no GPU, no window). Only compiled for tests.

use std::collections::HashMap;
use std::path::Path;

use eframe::egui::{self, Color32, TextureId, epaint::Primitive};

pub struct Renderer {
    textures: HashMap<TextureId, (usize, usize, Vec<Color32>)>,
    pub width: usize,
    pub height: usize,
}

impl Renderer {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            textures: HashMap::new(),
            width,
            height,
        }
    }

    pub fn input(&self) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(self.width as f32, self.height as f32),
            )),
            ..Default::default()
        }
    }

    /// Run one frame (keeping the texture atlas up to date); returns the frame's output.
    pub fn frame(
        &mut self,
        ctx: &egui::Context,
        input: egui::RawInput,
        ui: impl FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        let mut out = ctx.run_ui(input, ui);
        for (id, deltas) in &out.textures_delta.set {
            for delta in deltas.iter() {
                let egui::ImageData::Color(img) = &delta.image;
                let [w, h] = img.size;
                match delta.pos {
                    None => {
                        self.textures.insert(*id, (w, h, img.pixels.clone()));
                    }
                    Some([x0, y0]) => {
                        if let Some((tw, _, px)) = self.textures.get_mut(id) {
                            for y in 0..h {
                                for x in 0..w {
                                    px[(y0 + y) * *tw + x0 + x] = img.pixels[y * w + x];
                                }
                            }
                        }
                    }
                }
            }
        }
        for id in &out.textures_delta.free {
            self.textures.remove(id);
        }
        out.textures_delta.clear();
        out
    }

    /// Rasterise a frame's shapes and write them as a PNG.
    pub fn save(&self, ctx: &egui::Context, out: egui::FullOutput, path: &Path) {
        let ppp = out.pixels_per_point;
        let prims = ctx.tessellate(out.shapes, ppp);
        let (w, h) = (self.width, self.height);
        let mut buf = vec![Color32::from_gray(30); w * h];
        for p in prims {
            let Primitive::Mesh(mesh) = p.primitive else {
                continue;
            };
            let tex = self.textures.get(&mesh.texture_id);
            let clip = p.clip_rect;
            let (cx0, cy0) = (
                (clip.min.x * ppp).max(0.0) as i32,
                (clip.min.y * ppp).max(0.0) as i32,
            );
            let (cx1, cy1) = (
                ((clip.max.x * ppp) as i32).min(w as i32),
                ((clip.max.y * ppp) as i32).min(h as i32),
            );
            for t in mesh.indices.chunks(3) {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| mesh.vertices[i as usize]);
                let (pa, pb, pc) = (a.pos * ppp, b.pos * ppp, c.pos * ppp);
                let area = (pb.x - pa.x) * (pc.y - pa.y) - (pc.x - pa.x) * (pb.y - pa.y);
                if area.abs() < 1e-6 {
                    continue;
                }
                let x0 = pa.x.min(pb.x).min(pc.x).floor().max(cx0 as f32) as i32;
                let x1 = pa.x.max(pb.x).max(pc.x).ceil().min(cx1 as f32) as i32;
                let y0 = pa.y.min(pb.y).min(pc.y).floor().max(cy0 as f32) as i32;
                let y1 = pa.y.max(pb.y).max(pc.y).ceil().min(cy1 as f32) as i32;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = ((pb.x - px) * (pc.y - py) - (pc.x - px) * (pb.y - py)) / area;
                        let w1 = ((pc.x - px) * (pa.y - py) - (pa.x - px) * (pc.y - py)) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                            continue;
                        }
                        let mix = |f: fn(Color32) -> u8| {
                            w0 * f(a.color) as f32 + w1 * f(b.color) as f32 + w2 * f(c.color) as f32
                        };
                        let mut col = [
                            mix(|c| c.r()),
                            mix(|c| c.g()),
                            mix(|c| c.b()),
                            mix(|c| c.a()),
                        ];
                        if let Some((tw, th, px_)) = tex {
                            let u = w0 * a.uv.x + w1 * b.uv.x + w2 * c.uv.x;
                            let v = w0 * a.uv.y + w1 * b.uv.y + w2 * c.uv.y;
                            let tx = ((u * *tw as f32) as usize).min(tw - 1);
                            let ty = ((v * *th as f32) as usize).min(th - 1);
                            let s = px_[ty * tw + tx];
                            let sc = [s.r(), s.g(), s.b(), s.a()];
                            for k in 0..4 {
                                col[k] = col[k] * sc[k] as f32 / 255.0;
                            }
                        }
                        // Premultiplied alpha "over".
                        let d = &mut buf[y as usize * w + x as usize];
                        let inv = 1.0 - col[3] / 255.0;
                        let blend = |s: f32, d: u8| (s + d as f32 * inv).clamp(0.0, 255.0) as u8;
                        *d = Color32::from_rgba_premultiplied(
                            blend(col[0], d.r()),
                            blend(col[1], d.g()),
                            blend(col[2], d.b()),
                            255,
                        );
                    }
                }
            }
        }
        let file = std::fs::File::create(path).expect("create png");
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().expect("png header");
        let data: Vec<u8> = buf.iter().flat_map(|c| [c.r(), c.g(), c.b()]).collect();
        writer.write_image_data(&data).expect("png data");
    }
}
