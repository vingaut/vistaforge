//! Wall-distance field used by the camera rig.
//!
//! A "wall" is a cell whose local slope exceeds 45° (rise/run > 1). The field
//! stores the approximate Euclidean distance to the nearest wall in metres,
//! the height of that wall and a smoothed gradient. The rig samples it ahead
//! of the point of view and steers away when the clearance drops below the
//! configured threshold.
//!
//! The raw chamfer field is a staircase: its gradient flips direction from one
//! cell to the next. Feeding that directly into the flight dynamics produced
//! the "frantic roll" reported on steep terrain. The distance is therefore
//! box-blurred and the gradient is sampled with a two-cell stencil, while the
//! wall height lets the rig ignore cliffs it flies well above.

use vf_core::HeightField;

/// Distance-to-wall field on the heightfield grid.
#[derive(Debug, Clone)]
pub struct ClearanceField {
    width: usize,
    height: usize,
    origin: (f64, f64),
    spacing: (f64, f64),
    distance: Vec<f32>,
    /// Top elevation of the nearest wall, metres. `NEG_INFINITY` when none.
    wall_top: Vec<f32>,
    max_distance: f32,
}

impl ClearanceField {
    pub fn new(hf: &HeightField) -> Self {
        let (sx, sy) = hf.spacing();
        let cell = sx.max(sy) as f32;
        let width = hf.width();
        let height = hf.height();
        let mut distance = vec![f32::INFINITY; width * height];
        let mut wall_top = vec![f32::NEG_INFINITY; width * height];
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
                    // A cliff cell is a wall up to its highest corner: the
                    // flight can still pass above it without evasive action.
                    let mut top = hf.get(x, y);
                    for (nx, ny) in [
                        (x.saturating_sub(1), y),
                        ((x + 1).min(width - 1), y),
                        (x, y.saturating_sub(1)),
                        (x, (y + 1).min(height - 1)),
                    ] {
                        top = top.max(hf.get(nx, ny));
                    }
                    wall_top[y * width + x] = top;
                    wall_count += 1;
                }
            }
        }
        if wall_count == 0 {
            distance.fill(f32::INFINITY);
        } else {
            chamfer(&mut distance, &mut wall_top, width, height, cell);
            blur_distance(&mut distance, width, height, 2);
        }
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
            wall_top,
            max_distance,
        }
    }

    pub fn max_distance(&self) -> f32 {
        self.max_distance
    }

    /// Sample distance (metres), the horizontal gradient pointing away from
    /// the nearest wall, and the top elevation of that wall.
    pub fn sample(&self, x: f64, y: f64) -> (f32, f32, f32, f32) {
        if self.max_distance.is_infinite() {
            return (f32::INFINITY, 0.0, 0.0, f32::NEG_INFINITY);
        }
        let (ox, oy) = self.origin;
        let (sx, sy) = self.spacing;
        let gx = ((x - ox) / sx).clamp(0.0, (self.width - 1) as f64);
        let gy = ((y - oy) / sy).clamp(0.0, (self.height - 1) as f64);
        let d = self.bilinear_distance(gx, gy).min(self.max_distance);
        // Two-cell stencil: a one-cell stencil follows the staircase of the
        // chamfer grid and produces a direction that flips at every cell.
        let eps = 2.0;
        let dx = (self.bilinear_distance(gx + eps, gy) - self.bilinear_distance(gx - eps, gy))
            / (2.0 * eps * sx) as f32;
        let dy = (self.bilinear_distance(gx, gy + eps) - self.bilinear_distance(gx, gy - eps))
            / (2.0 * eps * sy) as f32;
        let top = self.bilinear_top(gx, gy);
        (d, dx, dy, top)
    }

    fn bilinear_distance(&self, gx: f64, gy: f64) -> f32 {
        self.bilinear(&self.distance, gx, gy, self.max_distance)
    }

    fn bilinear_top(&self, gx: f64, gy: f64) -> f32 {
        let x0 = gx.floor() as isize;
        let y0 = gy.floor() as isize;
        let x0 = x0.clamp(0, self.width as isize - 1) as usize;
        let y0 = y0.clamp(0, self.height as isize - 1) as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = (gx - x0 as f64) as f32;
        let ty = (gy - y0 as f64) as f32;
        let w = self.width;
        let a = self.wall_top[y0 * w + x0]
            + (self.wall_top[y0 * w + x1] - self.wall_top[y0 * w + x0]) * tx;
        let b = self.wall_top[y1 * w + x0]
            + (self.wall_top[y1 * w + x1] - self.wall_top[y1 * w + x0]) * tx;
        a + (b - a) * ty
    }

    fn bilinear(&self, values: &[f32], gx: f64, gy: f64, fallback: f32) -> f32 {
        let x0 = gx.floor() as isize;
        let y0 = gy.floor() as isize;
        let x0 = x0.clamp(0, self.width as isize - 1) as usize;
        let y0 = y0.clamp(0, self.height as isize - 1) as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let tx = (gx - x0 as f64) as f32;
        let ty = (gy - y0 as f64) as f32;
        let w = self.width;
        let v00 = values[y0 * w + x0];
        let v10 = values[y0 * w + x1];
        let v01 = values[y1 * w + x0];
        let v11 = values[y1 * w + x1];
        let a = v00 + (v10 - v00) * tx;
        let b = v01 + (v11 - v01) * tx;
        let value = a + (b - a) * ty;
        if value.is_finite() { value } else { fallback }
    }
}

fn chamfer(distance: &mut [f32], wall_top: &mut [f32], width: usize, height: usize, cell: f32) {
    let diag = std::f32::consts::SQRT_2 * cell;
    let orth = cell;
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            if x > 0 {
                relax(distance, wall_top, i, i - 1, orth);
            }
            if y > 0 {
                relax(distance, wall_top, i, i - width, orth);
                if x > 0 {
                    relax(distance, wall_top, i, i - width - 1, diag);
                }
                if x + 1 < width {
                    relax(distance, wall_top, i, i - width + 1, diag);
                }
            }
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let i = y * width + x;
            if x + 1 < width {
                relax(distance, wall_top, i, i + 1, orth);
            }
            if y + 1 < height {
                relax(distance, wall_top, i, i + width, orth);
                if x + 1 < width {
                    relax(distance, wall_top, i, i + width + 1, diag);
                }
                if x > 0 {
                    relax(distance, wall_top, i, i + width - 1, diag);
                }
            }
        }
    }
}

fn relax(distance: &mut [f32], wall_top: &mut [f32], target: usize, source: usize, cost: f32) {
    if distance[source].is_finite() {
        let candidate = distance[source] + cost;
        if candidate + 1e-6 < distance[target] {
            distance[target] = candidate;
            wall_top[target] = wall_top[source];
        }
    }
}

/// Separable box blur of the distance field. This removes the per-cell
/// staircase that made the avoidance direction jitter.
fn blur_distance(distance: &mut [f32], width: usize, height: usize, radius: usize) {
    if radius == 0 || width < 2 || height < 2 {
        return;
    }
    let mut temp = vec![0.0f32; distance.len()];
    let window = (radius * 2 + 1) as f32;
    for y in 0..height {
        let row = y * width;
        for x in 0..width {
            let mut sum = 0.0;
            for k in 0..=radius * 2 {
                let sample = (x + k).saturating_sub(radius).min(width - 1);
                sum += distance[row + sample];
            }
            temp[row + x] = sum / window;
        }
    }
    for y in 0..height {
        for x in 0..width {
            let mut sum = 0.0;
            for k in 0..=radius * 2 {
                let sample = (y + k).saturating_sub(radius).min(height - 1);
                sum += temp[sample * width + x];
            }
            distance[y * width + x] = sum / window;
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
        let (d, _, _, _) = field.sample(300.0, 300.0);
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
        let (near, _, _, top) = field.sample(310.0, 300.0);
        let (far, _, _, _) = field.sample(600.0, 300.0);
        assert!(near < far, "{near} / {far}");
        assert!(near < 60.0);
        assert!((top - 100.0).abs() < 1.0, "wall top {top}");
    }

    #[test]
    fn cliff_gradient_is_smooth_and_points_away() {
        let mut heights = vec![0.0f32; 65 * 65];
        for y in 0..65 {
            for x in 32..65 {
                heights[y * 65 + x] = 100.0;
            }
        }
        let hf = HeightField::new(65, 65, 10.0, 10.0, heights).unwrap();
        let field = ClearanceField::new(&hf);
        let mut previous: Option<f32> = None;
        for x in 150..290 {
            let (d, gx, _, _) = field.sample(f64::from(x), 300.0);
            assert!(d.is_finite());
            // The wall is east: the gradient must point west, consistently.
            assert!(gx < 0.0, "gradient at x={x}: {gx}");
            if let Some(previous) = previous {
                assert!(
                    (gx - previous).abs() < 0.05,
                    "gradient jumps from {previous} to {gx}"
                );
            }
            previous = Some(gx);
        }
    }
}
