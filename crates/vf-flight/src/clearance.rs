//! Wall-distance field used by the camera rig.
//!
//! A "wall" is a cell whose local slope exceeds 45° (rise/run > 1). The field
//! stores the approximate Euclidean distance to the nearest wall in metres and
//! its gradient, computed with a two-pass chamfer transform. The rig samples
//! it ahead of the point of view and pushes laterally away from the wall when
//! the clearance drops below the configured threshold.

use vf_core::HeightField;

/// Distance-to-wall field on the heightfield grid.
#[derive(Debug, Clone)]
pub struct ClearanceField {
    width: usize,
    height: usize,
    origin: (f64, f64),
    spacing: (f64, f64),
    distance: Vec<f32>,
    max_distance: f32,
}

impl ClearanceField {
    pub fn new(hf: &HeightField) -> Self {
        let (sx, sy) = hf.spacing();
        let cell = sx.max(sy) as f32;
        let width = hf.width();
        let height = hf.height();
        let mut distance = vec![f32::INFINITY; width * height];
        let mut wall_count = 0usize;
        for y in 0..height {
            for x in 0..width {
                let (gx, gy) = hf.gradient_at(x, y);
                let slope = (gx * gx + gy * gy).sqrt();
                let steep = slope > 1.0;
                // A one-cell escarpment also counts even when the gradient is
                // split between two cells.
                let mut relief = 0.0f32;
                if x > 0 {
                    relief = relief.max((hf.get(x, y) - hf.get(x - 1, y)).abs());
                }
                if x + 1 < width {
                    relief = relief.max((hf.get(x + 1, y) - hf.get(x, y)).abs());
                }
                if y > 0 {
                    relief = relief.max((hf.get(x, y) - hf.get(x, y - 1)).abs());
                }
                if y + 1 < height {
                    relief = relief.max((hf.get(x, y + 1) - hf.get(x, y)).abs());
                }
                if steep || relief > cell * 1.25 {
                    distance[y * width + x] = 0.0;
                    wall_count += 1;
                }
            }
        }
        if wall_count == 0 {
            distance.fill(f32::INFINITY);
        }
        chamfer(&mut distance, width, height, cell);
        let max_distance = if wall_count == 0 {
            f32::INFINITY
        } else {
            distance
                .iter()
                .copied()
                .filter(|v| v.is_finite())
                .fold(0.0f32, f32::max)
        };
        Self {
            width,
            height,
            origin: hf.origin(),
            spacing: hf.spacing(),
            distance,
            max_distance,
        }
    }

    pub fn max_distance(&self) -> f32 {
        self.max_distance
    }

    /// Sample distance (metres) and the horizontal gradient at a world point.
    /// The gradient points away from the nearest wall.
    pub fn sample(&self, x: f64, y: f64) -> (f32, f32, f32) {
        if self.max_distance.is_infinite() {
            return (f32::INFINITY, 0.0, 0.0);
        }
        let (ox, oy) = self.origin;
        let (sx, sy) = self.spacing;
        let gx = ((x - ox) / sx).clamp(0.0, (self.width - 1) as f64);
        let gy = ((y - oy) / sy).clamp(0.0, (self.height - 1) as f64);
        let d = self.bilinear(gx, gy).min(self.max_distance);
        let eps = 1.0;
        let dx = (self.bilinear(gx + eps, gy) - self.bilinear(gx - eps, gy))
            / (2.0 * eps * sx) as f32;
        let dy = (self.bilinear(gx, gy + eps) - self.bilinear(gx, gy - eps))
            / (2.0 * eps * sy) as f32;
        (d, dx, dy)
    }

    fn bilinear(&self, gx: f64, gy: f64) -> f32 {
        let x0 = gx.floor() as usize;
        let y0 = gy.floor() as usize;
        let x0 = x0.min(self.width - 1);
        let y0 = y0.min(self.height - 1);
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = (gx - x0 as f64) as f32;
        let ty = (gy - y0 as f64) as f32;
        let w = self.width;
        let v00 = self.distance[y0 * w + x0];
        let v10 = self.distance[y0 * w + x1];
        let v01 = self.distance[y1 * w + x0];
        let v11 = self.distance[y1 * w + x1];
        let a = v00 + (v10 - v00) * tx;
        let b = v01 + (v11 - v01) * tx;
        let value = a + (b - a) * ty;
        if value.is_finite() { value } else { self.max_distance }
    }
}

fn chamfer(distance: &mut [f32], width: usize, height: usize, cell: f32) {
    let diag = std::f32::consts::SQRT_2 * cell;
    let orth = cell;
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            let mut d = distance[i];
            if x > 0 {
                d = d.min(distance[i - 1] + orth);
            }
            if y > 0 {
                d = d.min(distance[i - width] + orth);
                if x > 0 {
                    d = d.min(distance[i - width - 1] + diag);
                }
                if x + 1 < width {
                    d = d.min(distance[i - width + 1] + diag);
                }
            }
            distance[i] = d;
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let i = y * width + x;
            let mut d = distance[i];
            if x + 1 < width {
                d = d.min(distance[i + 1] + orth);
            }
            if y + 1 < height {
                d = d.min(distance[i + width] + orth);
                if x + 1 < width {
                    d = d.min(distance[i + width + 1] + diag);
                }
                if x > 0 {
                    d = d.min(distance[i + width - 1] + diag);
                }
            }
            distance[i] = d;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_terrain_has_no_walls() {
        let hf = HeightField::filled(65, 65, 10.0, 100.0).unwrap();
        let field = ClearanceField::new(&hf);
        assert!(field.max_distance().is_infinite() || field.max_distance() > 0.0);
        let (d, _, _) = field.sample(300.0, 300.0);
        assert!(d.is_infinite(), "a flat terrain has no wall");
    }

    #[test]
    fn a_cliff_creates_a_distance_field() {
        let mut heights = vec![0.0f32; 65 * 65];
        for y in 0..65 {
            for x in 32..65 {
                heights[y * 65 + x] = 100.0;
            }
        }
        let hf = HeightField::new(65, 65, 10.0, 10.0, heights).unwrap();
        let field = ClearanceField::new(&hf);
        // Near the cliff the distance is small; far away it is larger.
        let (near, _, _) = field.sample(310.0, 300.0);
        let (far, _, _) = field.sample(600.0, 300.0);
        assert!(near < far, "{near} / {far}");
        assert!(near < 60.0);
    }
}
