//! Deterministic hydraulic droplet erosion and thermal talus relaxation.

use vf_core::{ErosionParams, HeightField, Result, Seed};

/// Erode a heightfield in place and return a new field with the same metadata.
pub fn erode(hf: &HeightField, params: &ErosionParams, seed: Seed) -> Result<HeightField> {
    params.validate().map_err(vf_core::Error::Invalid)?;
    if !params.hydraulic && !params.thermal {
        return Ok(hf.clone());
    }
    let mut heights = hf.heights().to_vec();
    let width = hf.width();
    let height = hf.height();
    if params.hydraulic {
        hydraulic(
            &mut heights,
            width,
            height,
            params,
            seed.derive("hydraulic"),
        );
    }
    if params.thermal {
        thermal(
            &mut heights,
            width,
            height,
            params.talus_angle_deg,
            params.thermal_iterations,
            seed.derive("thermal"),
        );
    }
    let mut out = HeightField::new(width, height, hf.spacing().0, hf.spacing().1, heights)?;
    let (ox, oy) = hf.origin();
    out.set_origin(ox, oy);
    out.set_nodata(hf.nodata());
    if let Some(mask) = hf.mask() {
        out.set_mask(Some(mask.to_vec()))?;
    }
    Ok(out)
}

fn hydraulic(heights: &mut [f32], width: usize, height: usize, params: &ErosionParams, seed: Seed) {
    if width < 3 || height < 3 || params.droplets == 0 {
        return;
    }
    let mut rng = seed.rng();
    let max_x = (width - 2) as f64;
    let max_y = (height - 2) as f64;
    let lifetime = params.max_lifetime.max(1);
    let inertia = f64::from(params.inertia.clamp(0.0, 1.0));
    let gravity = f64::from(params.gravity.max(0.0));
    let capacity_factor = f64::from(params.capacity.max(0.0));
    let erosion_rate = f64::from(params.erosion_rate.clamp(0.0, 1.0));
    let deposition_rate = f64::from(params.deposition_rate.clamp(0.0, 1.0));
    let evaporation = f64::from(params.evaporation.clamp(0.0, 1.0));

    for _ in 0..params.droplets {
        let mut pos_x = 1.0 + rng.next_f64() * max_x;
        let mut pos_y = 1.0 + rng.next_f64() * max_y;
        let mut dir_x = 0.0f64;
        let mut dir_y = 0.0f64;
        let mut speed = 1.0f64;
        let mut water = 1.0f64;
        let mut sediment = 0.0f64;

        for _ in 0..lifetime {
            let (h, gx, gy) = sample_with_gradient(heights, width, height, pos_x, pos_y);
            dir_x = dir_x * inertia - gx * (1.0 - inertia);
            dir_y = dir_y * inertia - gy * (1.0 - inertia);
            let len = (dir_x * dir_x + dir_y * dir_y).sqrt();
            if len < 1e-9 {
                let angle = rng.next_f64() * std::f64::consts::TAU;
                dir_x = angle.cos();
                dir_y = angle.sin();
            } else {
                dir_x /= len;
                dir_y /= len;
            }
            let next_x = pos_x + dir_x;
            let next_y = pos_y + dir_y;
            if next_x < 1.0
                || next_y < 1.0
                || next_x > (width - 2) as f64
                || next_y > (height - 2) as f64
            {
                break;
            }
            let h_next = sample_bilinear(heights, width, height, next_x, next_y);
            let dh = f64::from(h_next - h);
            let capacity = (-dh).max(0.001) * speed * water * capacity_factor;

            if sediment > capacity || dh > 0.0 {
                let amount = if dh > 0.0 {
                    sediment.min(dh)
                } else {
                    (sediment - capacity) * deposition_rate
                };
                if amount > 0.0 {
                    deposit(heights, width, height, pos_x, pos_y, amount as f32);
                    sediment -= amount;
                }
            } else {
                let amount = ((capacity - sediment) * erosion_rate).min(-dh);
                if amount > 0.0 {
                    erode_at(heights, width, height, pos_x, pos_y, amount as f32);
                    sediment += amount;
                }
            }

            speed = (speed * speed + (-dh) * gravity).max(0.0).sqrt();
            water *= 1.0 - evaporation;
            pos_x = next_x;
            pos_y = next_y;
        }
    }
}

#[inline]
fn sample_bilinear(data: &[f32], width: usize, height: usize, x: f64, y: f64) -> f32 {
    let x = x.clamp(0.0, (width - 1) as f64);
    let y = y.clamp(0.0, (height - 1) as f64);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = (x - x0 as f64) as f32;
    let ty = (y - y0 as f64) as f32;
    let a = data[y0 * width + x0] + (data[y0 * width + x1] - data[y0 * width + x0]) * tx;
    let b = data[y1 * width + x0] + (data[y1 * width + x1] - data[y1 * width + x0]) * tx;
    a + (b - a) * ty
}

#[inline]
fn sample_with_gradient(
    data: &[f32],
    width: usize,
    height: usize,
    x: f64,
    y: f64,
) -> (f32, f64, f64) {
    let h = sample_bilinear(data, width, height, x, y);
    let gx = (sample_bilinear(data, width, height, x + 0.75, y)
        - sample_bilinear(data, width, height, x - 0.75, y)) as f64
        / 1.5;
    let gy = (sample_bilinear(data, width, height, x, y + 0.75)
        - sample_bilinear(data, width, height, x, y - 0.75)) as f64
        / 1.5;
    (h, gx, gy)
}

fn deposit(data: &mut [f32], width: usize, height: usize, x: f64, y: f64, amount: f32) {
    let x0 = x.floor().clamp(0.0, (width - 1) as f64) as usize;
    let y0 = y.floor().clamp(0.0, (height - 1) as f64) as usize;
    let x1 = (x0 + 1).min(width - 1);
    let y1 = (y0 + 1).min(height - 1);
    let tx = (x - x0 as f64) as f32;
    let ty = (y - y0 as f64) as f32;
    let w00 = (1.0 - tx) * (1.0 - ty);
    let w10 = tx * (1.0 - ty);
    let w01 = (1.0 - tx) * ty;
    let w11 = tx * ty;
    data[y0 * width + x0] += amount * w00;
    data[y0 * width + x1] += amount * w10;
    data[y1 * width + x0] += amount * w01;
    data[y1 * width + x1] += amount * w11;
}

fn erode_at(data: &mut [f32], width: usize, height: usize, x: f64, y: f64, amount: f32) {
    const RADIUS: f64 = 2.0;
    let x0 = (x - RADIUS).floor().max(0.0) as usize;
    let x1 = (x + RADIUS).ceil().min((width - 1) as f64) as usize;
    let y0 = (y - RADIUS).floor().max(0.0) as usize;
    let y1 = (y + RADIUS).ceil().min((height - 1) as f64) as usize;
    let mut weights = Vec::new();
    let mut total = 0.0f32;
    for py in y0..=y1 {
        for px in x0..=x1 {
            let dx = px as f64 - x;
            let dy = py as f64 - y;
            let d = (dx * dx + dy * dy).sqrt();
            if d < RADIUS {
                let w = ((RADIUS - d) / RADIUS) as f32;
                weights.push((py * width + px, w));
                total += w;
            }
        }
    }
    if total <= 0.0 {
        return;
    }
    for (i, w) in weights {
        data[i] -= amount * w / total;
    }
}

fn thermal(
    data: &mut [f32],
    width: usize,
    height: usize,
    angle_deg: f32,
    iterations: u32,
    _seed: Seed,
) {
    let talus = angle_deg.to_radians().tan().max(1e-4);
    let mut delta = vec![0.0f32; data.len()];
    for _ in 0..iterations.max(1) {
        delta.fill(0.0);
        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let h = data[y * width + x];
                for (dx, dy) in [
                    (-1i32, -1i32),
                    (0, -1),
                    (1, -1),
                    (-1, 0),
                    (1, 0),
                    (-1, 1),
                    (0, 1),
                    (1, 1),
                ] {
                    let nx = (x as i32 + dx) as usize;
                    let ny = (y as i32 + dy) as usize;
                    let nh = data[ny * width + nx];
                    let distance = ((dx * dx + dy * dy) as f32).sqrt();
                    let diff = h - nh;
                    if diff > talus * distance {
                        let move_amount = (diff - talus * distance) * 0.125;
                        let i = y * width + x;
                        let j = ny * width + nx;
                        delta[i] -= move_amount;
                        delta[j] += move_amount;
                    }
                }
            }
        }
        for (h, d) in data.iter_mut().zip(&delta) {
            *h += *d;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cone(size: usize) -> HeightField {
        let mut heights = vec![0.0f32; size * size];
        let c = (size - 1) as f32 * 0.5;
        for y in 0..size {
            for x in 0..size {
                let d = ((x as f32 - c).hypot(y as f32 - c)) / c;
                heights[y * size + x] = (1.0 - d).max(0.0) * 1000.0;
            }
        }
        HeightField::new(size, size, 30.0, 30.0, heights).unwrap()
    }

    #[test]
    fn erosion_is_deterministic_and_changes_relief() {
        let hf = cone(65);
        let params = ErosionParams {
            droplets: 3_000,
            thermal_iterations: 4,
            ..ErosionParams::default()
        };
        let a = erode(&hf, &params, Seed::new(11)).unwrap();
        let b = erode(&hf, &params, Seed::new(11)).unwrap();
        assert_eq!(a.heights(), b.heights());
        assert_ne!(a.heights(), hf.heights());
        assert!(a.stats().std_dev <= hf.stats().std_dev + 1.0);
    }

    #[test]
    fn flat_field_is_unchanged() {
        let hf = HeightField::filled(32, 32, 30.0, 500.0).unwrap();
        let params = ErosionParams {
            droplets: 200,
            thermal_iterations: 2,
            ..ErosionParams::default()
        };
        let out = erode(&hf, &params, Seed::new(1)).unwrap();
        assert!(out.heights().iter().all(|&v| (v - 500.0).abs() < 1e-3));
    }
}
