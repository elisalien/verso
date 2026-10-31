//! MilkDrop's preset blend: a per-vertex pattern (a wipe, a plasma or a
//! radial sweep) decides where the new preset takes over first. Port of
//! Butterchurn's `blendPattern`.

use crate::eel::Rng;
use crate::engine::{COMP_HEIGHT, COMP_WIDTH, MESH_HEIGHT, MESH_WIDTH};

const W1: usize = MESH_WIDTH + 1;
const H1: usize = MESH_HEIGHT + 1;

/// Per-vertex blend weights over the warp mesh.
#[derive(Debug, Clone)]
pub struct BlendPattern {
    a: Vec<f32>,
    c: Vec<f32>,
}

impl BlendPattern {
    /// A random pattern for a `width` × `height` buffer.
    pub fn new(width: u32, height: u32, rng: &mut Rng) -> Self {
        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        let aspectx = if h > w { w / h } else { 1.0 };
        let aspecty = if w > h { h / w } else { 1.0 };
        let mut p = BlendPattern {
            a: vec![1.0; W1 * H1],
            c: vec![0.0; W1 * H1],
        };
        let rand = |rng: &mut Rng| rng.next_f64() as f32;
        match 1 + (rand(rng) * 3.0) as u32 {
            1 => {
                // Straight wipe in a random direction.
                let ang = rand(rng) * std::f32::consts::TAU;
                let (vx, vy) = (ang.cos(), ang.sin());
                let band = 0.1 + 0.2 * rand(rng);
                for y in 0..H1 {
                    let fy = y as f32 / MESH_HEIGHT as f32 * aspecty;
                    for x in 0..W1 {
                        let fx = x as f32 / MESH_WIDTH as f32 * aspectx;
                        let t = (fx - 0.5) * vx + (fy - 0.5) * vy + 0.5;
                        p.set(y * W1 + x, band, (t - 0.5) / 2f32.sqrt() + 0.5);
                    }
                }
            }
            2 => {
                // Plasma.
                let band = 0.12 + 0.13 * rand(rng);
                for i in [0, MESH_WIDTH, MESH_HEIGHT * W1, MESH_HEIGHT * W1 + MESH_WIDTH] {
                    p.c[i] = rand(rng);
                }
                p.plasma(0, MESH_WIDTH, 0, MESH_HEIGHT, 0.25, aspectx, aspecty, rng);
                let min = p.c.iter().cloned().fold(f32::MAX, f32::min);
                let max = p.c.iter().cloned().fold(f32::MIN, f32::max);
                let mult = 1.0 / (max - min).max(1e-6);
                for i in 0..W1 * H1 {
                    let t = (p.c[i] - min) * mult;
                    p.set(i, band, t);
                }
            }
            _ => {
                // Radial, inward or outward.
                let band = 0.02 + 0.14 * rand(rng) + 0.34 * rand(rng);
                let inward = rand(rng) < 0.5;
                for y in 0..H1 {
                    let dy = (y as f32 / MESH_HEIGHT as f32 - 0.5) * aspecty;
                    for x in 0..W1 {
                        let dx = (x as f32 / MESH_WIDTH as f32 - 0.5) * aspectx;
                        let t = (dx * dx + dy * dy).sqrt() * std::f32::consts::SQRT_2;
                        p.set(y * W1 + x, band, if inward { 1.0 - t } else { t });
                    }
                }
            }
        }
        p
    }

    fn set(&mut self, i: usize, band: f32, t: f32) {
        let inv = 1.0 / band;
        self.a[i] = inv * (1.0 + band);
        self.c[i] = -inv + inv * t;
    }

    #[allow(clippy::too_many_arguments)]
    fn plasma(&mut self, x0: usize, x1: usize, y0: usize, y1: usize, dt: f32, ax: f32, ay: f32, rng: &mut Rng) {
        let mut r = || rng.next_f64() as f32 * 2.0 - 1.0;
        let (midx, midy) = ((x0 + x1) / 2, (y0 + y1) / 2);
        let at = |c: &Vec<f32>, x: usize, y: usize| c[y * W1 + x];
        let (t00, t01, t10, t11) = (at(&self.c, x0, y0), at(&self.c, x1, y0), at(&self.c, x0, y1), at(&self.c, x1, y1));
        if y1 - y0 >= 2 {
            if x0 == 0 {
                self.c[midy * W1 + x0] = 0.5 * (t00 + t10) + r() * dt * ay;
            }
            self.c[midy * W1 + x1] = 0.5 * (t01 + t11) + r() * dt * ay;
        }
        if x1 - x0 >= 2 {
            if y0 == 0 {
                self.c[y0 * W1 + midx] = 0.5 * (t00 + t01) + r() * dt * ax;
            }
            self.c[y1 * W1 + midx] = 0.5 * (t10 + t11) + r() * dt * ax;
        }
        if y1 - y0 >= 2 && x1 - x0 >= 2 {
            let sum = at(&self.c, x0, midy) + at(&self.c, x1, midy) + at(&self.c, midx, y0) + at(&self.c, midx, y1);
            self.c[midy * W1 + midx] = 0.25 * sum + r() * dt;
            self.plasma(x0, midx, y0, midy, dt * 0.5, ax, ay, rng);
            self.plasma(midx, x1, y0, midy, dt * 0.5, ax, ay, rng);
            self.plasma(x0, midx, midy, y1, dt * 0.5, ax, ay, rng);
            self.plasma(midx, x1, midy, y1, dt * 0.5, ax, ay, rng);
        }
    }

    /// Weight of the new preset at warp mesh vertex `i` (row by row from the
    /// top), for blend `progress` in 0..1.
    pub fn weight(&self, i: usize, progress: f32) -> f32 {
        (self.a[i] * progress + self.c[i]).clamp(0.0, 1.0)
    }

    /// Weights at the composite mesh vertices, interpolated from the warp
    /// mesh as Butterchurn does.
    pub fn comp_weights(&self, progress: f32) -> Vec<f32> {
        let mut out = Vec::with_capacity((COMP_WIDTH + 1) * (COMP_HEIGHT + 1));
        for j in 0..=COMP_HEIGHT {
            for i in 0..=COMP_WIDTH {
                let x = (i as f32 / COMP_WIDTH as f32 * W1 as f32).clamp(0.0, MESH_WIDTH as f32 - 1.0);
                let y = (j as f32 / COMP_HEIGHT as f32 * H1 as f32).clamp(0.0, MESH_HEIGHT as f32 - 1.0);
                let (nx, ny) = (x.floor() as usize, y.floor() as usize);
                let (dx, dy) = (x - nx as f32, y - ny as f32);
                let w = |x: usize, y: usize| self.weight(y * W1 + x, progress);
                out.push(
                    w(nx, ny) * (1.0 - dx) * (1.0 - dy)
                        + w(nx + 1, ny) * dx * (1.0 - dy)
                        + w(nx, ny + 1) * (1.0 - dx) * dy
                        + w(nx + 1, ny + 1) * dx * dy,
                );
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_goes_from_old_to_new() {
        for seed in 1..20 {
            let p = BlendPattern::new(640, 360, &mut Rng::new(seed));
            // Like MilkDrop, corners may lead or lag slightly; on average the
            // blend runs from the old preset to the new one.
            let mean = |t: f32| (0..W1 * H1).map(|i| p.weight(i, t)).sum::<f32>() / (W1 * H1) as f32;
            assert!(mean(0.0) < 0.1, "seed {seed} starts at {}", mean(0.0));
            assert!(mean(1.0) > 0.9, "seed {seed} ends at {}", mean(1.0));
            assert!((0..W1 * H1).all(|i| p.weight(i, 0.6) >= p.weight(i, 0.3)));
            assert_eq!(p.comp_weights(0.5).len(), (COMP_WIDTH + 1) * (COMP_HEIGHT + 1));
        }
    }
}
