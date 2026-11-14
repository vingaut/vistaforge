//! Classic midpoint displacement (diamond-square).
//!
//! The exact 1994 RNG and amplitude schedule are unknowable from the binary;
//! this implementation reproduces the observable style and exposes the same
//! parameters (seed, FrDim, divisor, island, vertical scale).

use vf_core::{ClassicParams, HeightField, Result, Seed};
use vf_terrain::{ResampleFilter, resample};

use crate::DEFAULT_WORLD_EXTENT_M;

/// Generate a classic fractal terrain.
pub fn generate(params: &ClassicParams) -> Result<HeightField> {
    params.validate().map_err(vf_core::Error::Invalid)?;

    let size = params.size as usize;
    let native = size >= 3 && (size - 2).is_power_of_two();
    let base = if native {
        size - 1
    } else {
        (size - 1).next_power_of_two() + 1
    };
    let mut grid = diamond_square(base, params);

    normalize_and_scale(
        &mut grid,
        base,
        params.island,
        params.vertical_scale * params.roughness,
    );

    let spacing = DEFAULT_WORLD_EXTENT_M / (base - 1) as f64;
    let mut hf = HeightField::new(base, base, spacing, spacing, grid)?;
    if base != size {
        hf = if native {
            pad_border(&hf, size)?
        } else {
            resample(&hf, size, size, ResampleFilter::Bicubic)?
        };
    }
    hf.set_origin(0.0, 0.0);
    Ok(hf)
}

fn pad_border(hf: &HeightField, size: usize) -> Result<HeightField> {
    let base = hf.width();
    let mut heights = Vec::with_capacity(size * size);
    for y in 0..size {
        let sy = y.min(base - 1);
        for x in 0..size {
            let sx = x.min(base - 1);
            heights.push(hf.get(sx, sy));
        }
    }
    let mut out = HeightField::new(size, size, hf.spacing().0, hf.spacing().1, heights)?;
    out.set_origin(hf.origin().0, hf.origin().1);
    Ok(out)
}

fn diamond_square(base: usize, params: &ClassicParams) -> Vec<f32> {
    debug_assert!(base >= 2 && (base - 1).is_power_of_two());
    let mut grid = vec![0.0f32; base * base];
    let mut rng = Seed::new(params.seed).rng();

    // Initial lattice: all points on a coarse grid get random values. A larger
    // divisor starts finer, producing many smaller mountains.
    let initial_step = ((base - 1) / usize::from(params.divisor)).max(1);
    for y in (0..base).step_by(initial_step) {
        for x in (0..base).step_by(initial_step) {
            grid[y * base + x] = rng.signed();
        }
    }
    // Ensure the far edges are filled even when the grid does not divide evenly.
    for i in 0..base {
        let last = base - 1;
        grid[last * base + i] = if grid[last * base + i] == 0.0 {
            rng.signed()
        } else {
            grid[last * base + i]
        };
        grid[i * base + last] = if grid[i * base + last] == 0.0 {
            rng.signed()
        } else {
            grid[i * base + last]
        };
    }

    let dimension = params.fractal_dimension.clamp(0.0, 200.0) / 200.0;
    let roughness = (0.22 + 0.62 * dimension).clamp(0.05, 0.95);
    let mut amplitude = 1.0f32;
    let mut step = initial_step;

    while step > 1 {
        let half = step / 2;

        // Diamond phase: centre of each square.
        let mut y = 0;
        while y + step < base {
            let mut x = 0;
            while x + step < base {
                let avg = (grid[y * base + x]
                    + grid[y * base + x + step]
                    + grid[(y + step) * base + x]
                    + grid[(y + step) * base + x + step])
                    * 0.25;
                let cx = x + half;
                let cy = y + half;
                grid[cy * base + cx] = avg + rng.signed() * amplitude;
                x += step;
            }
            y += step;
        }

        // Square phase: edge midpoints. Samples outside the grid are clamped.
        let h = half as isize;
        let mut y = 0;
        while y < base {
            let mut x = 0;
            while x < base {
                if x + half < base {
                    let px = x + half;
                    let py = y;
                    let avg = (sample_off(&grid, base, px, py, -h, 0)
                        + sample_off(&grid, base, px, py, h, 0)
                        + sample_off(&grid, base, px, py, 0, -h)
                        + sample_off(&grid, base, px, py, 0, h))
                        * 0.25;
                    grid[py * base + px] = avg + rng.signed() * amplitude;
                }
                if y + half < base {
                    let px = x;
                    let py = y + half;
                    let avg = (sample_off(&grid, base, px, py, 0, -h)
                        + sample_off(&grid, base, px, py, 0, h)
                        + sample_off(&grid, base, px, py, -h, 0)
                        + sample_off(&grid, base, px, py, h, 0))
                        * 0.25;
                    grid[py * base + px] = avg + rng.signed() * amplitude;
                }
                x += step;
            }
            y += step;
        }

        amplitude *= roughness;
        step = half;
    }

    grid
}

#[inline]
fn sample_off(grid: &[f32], base: usize, x: usize, y: usize, dx: isize, dy: isize) -> f32 {
    let x = (x as isize + dx).clamp(0, base as isize - 1) as usize;
    let y = (y as isize + dy).clamp(0, base as isize - 1) as usize;
    grid[y * base + x]
}

fn apply_island(grid: &mut [f32], base: usize, island: bool) {
    if !island {
        return;
    }
    let center = (base - 1) as f32 * 0.5;
    let inv = 1.0 / center.max(1.0);
    for y in 0..base {
        for x in 0..base {
            let dx = (x as f32 - center) * inv;
            let dy = (y as f32 - center) * inv;
            let r2 = dx * dx + dy * dy;
            let falloff = (1.0 - r2).max(0.0).powf(1.4);
            grid[y * base + x] *= falloff;
        }
    }
    // Force the rim to a common altitude (sea level after normalisation).
    for i in 0..base {
        grid[i] = 0.0;
        grid[(base - 1) * base + i] = 0.0;
        grid[i * base] = 0.0;
        grid[i * base + base - 1] = 0.0;
    }
}

fn normalize_and_scale(grid: &mut [f32], base: usize, island: bool, vertical_scale: f32) {
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for &v in grid.iter() {
        min = min.min(v);
        max = max.max(v);
    }
    let range = (max - min).max(1e-9);
    for v in grid.iter_mut() {
        *v = (*v - min) / range;
    }
    apply_island(grid, base, island);
    let scale = vertical_scale.max(0.0);
    for v in grid.iter_mut() {
        *v *= scale;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_sizes_are_exact_and_deterministic() {
        for size in [258u32, 514, 1026] {
            let params = ClassicParams::default().with_size_seed(size, 42);
            let a = generate(&params).unwrap();
            let b = generate(&params).unwrap();
            assert_eq!((a.width(), a.height()), (size as usize, size as usize));
            assert_eq!(a.heights(), b.heights());
            assert_eq!(a.stats().min, 0.0);
            assert!((a.stats().max - params.vertical_scale).abs() < 1e-3);
        }
    }

    #[test]
    fn seeds_and_divisors_differ() {
        let base = ClassicParams::default().with_size_seed(258, 1);
        let a = generate(&base).unwrap();
        let b = generate(&ClassicParams {
            seed: 2,
            ..base.clone()
        })
        .unwrap();
        assert_ne!(a.heights(), b.heights());
        let c = generate(&ClassicParams { divisor: 8, ..base }).unwrap();
        assert_ne!(a.heights(), c.heights());
    }

    #[test]
    fn island_borders_are_at_sea_level() {
        let params = ClassicParams {
            island: true,
            ..ClassicParams::default().with_size_seed(258, 5)
        };
        let hf = generate(&params).unwrap();
        for i in 0..hf.width() {
            assert_eq!(hf.get(i, 0), 0.0);
            assert_eq!(hf.get(i, hf.height() - 1), 0.0);
            assert_eq!(hf.get(0, i), 0.0);
            assert_eq!(hf.get(hf.width() - 1, i), 0.0);
        }
    }
}
