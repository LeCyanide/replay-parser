//! Synthetic "minimap-like" RGBA frames: static gradient map with grid lines and
//! dense text-like glyph rows, plus moving icons and a moving label.
//!
//! Orientation/colour markers: solid red square top-left, green top-right,
//! blue bottom-left, white bottom-right.

pub struct Synth {
    pub width: usize,
    pub height: usize,
    background: Vec<u8>,
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

/// Smooth 2-D value noise in [0, 1).
fn value_noise(x: f32, y: f32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - xi as f32, y - yi as f32);
    let corner = |dx: i32, dy: i32| {
        let seed = ((xi + dx) as u32).wrapping_mul(73856093) ^ ((yi + dy) as u32).wrapping_mul(19349663);
        (hash(seed) & 0xffff) as f32 / 65536.0
    };
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (smooth(fx), smooth(fy));
    let top = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * sx;
    let bottom = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * sx;
    top + (bottom - top) * sy
}

impl Synth {
    pub fn new(width: usize, height: usize) -> Self {
        let mut s = Synth { width, height, background: vec![0; width * height * 4] };
        for y in 0..height {
            for x in 0..width {
                let i = (y * width + x) * 4;
                // Textured sea with "islands" (value noise), gradient, grid every 60 px.
                let t = (x + y) as f32 / (width + height) as f32;
                let n = value_noise(x as f32 / 90.0, y as f32 / 90.0) * 0.7
                    + value_noise(x as f32 / 12.0, y as f32 / 12.0) * 0.3;
                let grain = (hash((y * 7919 + x) as u32) & 7) as f32 - 3.5; // fine painted texture
                let mut px = if n > 0.62 {
                    let k = (n - 0.62) * 300.0;
                    [(150.0 + k * 0.3 + grain) as u8, (140.0 + k * 0.2 + grain) as u8, (90.0 + grain) as u8, 255]
                } else {
                    [
                        (20.0 + 60.0 * t + 40.0 * n + grain) as u8,
                        (60.0 + 50.0 * t + 40.0 * n + grain) as u8,
                        (110.0 - 20.0 * t + 30.0 * n + grain) as u8,
                        255,
                    ]
                };
                if x % 60 == 0 || y % 60 == 0 {
                    px = [90, 110, 130, 255];
                }
                s.background[i..i + 4].copy_from_slice(&px);
            }
        }
        // Rows of pseudo text (random 5x7 glyphs, 1px stroke) - high frequency detail.
        for row in 0..6 {
            let y0 = 80 + row * 14;
            for col in 0..60 {
                let x0 = 100 + col * 7;
                let bits = hash((row * 1000 + col) as u32);
                for gy in 0..7 {
                    for gx in 0..5 {
                        if bits >> ((gy * 5 + gx) % 32) & 1 == 1 {
                            s.set(&mut None, x0 + gx, y0 + gy, [255, 255, 255]);
                        }
                    }
                }
            }
        }
        let (w, h) = (width, height);
        s.fill_rect_bg(0, 0, 40, 40, [255, 0, 0]);
        s.fill_rect_bg(w - 40, 0, 40, 40, [0, 255, 0]);
        s.fill_rect_bg(0, h - 40, 40, 40, [0, 0, 255]);
        s.fill_rect_bg(w - 40, h - 40, 40, 40, [255, 255, 255]);
        s
    }

    fn set(&mut self, frame: &mut Option<&mut [u8]>, x: usize, y: usize, rgb: [u8; 3]) {
        if x < self.width && y < self.height {
            let i = (y * self.width + x) * 4;
            let buf: &mut [u8] = match frame {
                Some(f) => f,
                None => &mut self.background,
            };
            buf[i..i + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }

    fn fill_rect_bg(&mut self, x0: usize, y0: usize, w: usize, h: usize, rgb: [u8; 3]) {
        for y in y0..y0 + h {
            for x in x0..x0 + w {
                self.set(&mut None, x, y, rgb);
            }
        }
    }

    /// Renders frame `n` into `out` (width*height*4 bytes).
    pub fn render(&mut self, n: usize, out: &mut [u8]) {
        out.copy_from_slice(&self.background);
        let mut frame = Some(out);
        let t = n as f32;
        // 24 small "ship" icons moving on circles/lines.
        for k in 0..24 {
            let kf = k as f32;
            let cx = (self.width as f32 / 2.0 + (200.0 + 10.0 * kf) * (t * 0.01 + kf).cos()) as isize;
            let cy = (self.height as f32 / 2.0 + (150.0 + 5.0 * kf) * (t * 0.013 + kf * 0.7).sin()) as isize;
            let colour = if k % 2 == 0 { [80, 255, 120] } else { [255, 70, 70] };
            for dy in -6isize..=6 {
                for dx in -6isize..=6 {
                    if dx * dx + dy * dy <= 36 {
                        self.set(&mut frame, (cx + dx) as usize, (cy + dy) as usize, colour);
                    }
                }
            }
        }
        // A moving block of glyphs (a label following a ship).
        let lx = 200 + (n * 2) % self.width.saturating_sub(400).max(1);
        for col in 0..10 {
            let bits = hash(col as u32 + 77);
            for gy in 0..7 {
                for gx in 0..5 {
                    if bits >> ((gy * 5 + gx) % 32) & 1 == 1 {
                        self.set(&mut frame, lx + col * 7 + gx, 500 + gy, [255, 255, 0]);
                    }
                }
            }
        }
    }
}
