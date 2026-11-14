//! Tiled and seamless world composition.
//!
//! The stored heightfield is a *tile*. `compose` copies it into a larger grid
//! with flips/rotations and optional variation. `make_seamless` corrects the
//! four edges so a tile can repeat without a height step; the correction is a
//! smooth ramp localised in a band, which leaves the interior untouched.

use vf_core::{HeightField, HeightFieldError, Seed, WorldMode, WorldSettings};

/// Build the composed heightfield described by `settings`.
pub fn compose(
    source: &HeightField,
    settings: &WorldSettings,
    seed: Seed,
) -> Result<HeightField, String> {
    settings.validate()?;
    if settings.mode == WorldMode::Single {
        return Ok(source.clone());
    }
    let tile = if settings.mode == WorldMode::Seamless {
        make_seamless(source, settings.blend_cells as usize)?
    } else {
        source.clone()
    };
    let (w, h) = (tile.width(), tile.height());
    let (ow, oh) = settings.output_size(w, h);
    let tw = w - 1;
    let th = h - 1;
    // Rotations swap the roles of the four edges; they preserve a seam only
    // if all four edges match, which is not the case for a seamless tile.
    let square = tw == th && settings.mode != WorldMode::Seamless;
    let (sx, sy) = tile.spacing();
    let (ox, oy) = tile.origin();
    let tile_center = {
        let stats = tile.stats();
        (stats.min + stats.max) * 0.5
    };
    let mut heights = vec![0.0f32; ow * oh];
    for oy_index in 0..oh {
        let ty = (oy_index / th).min(settings.tiles_y as usize - 1);
        let v = oy_index - ty * th;
        for ox_index in 0..ow {
            let tx = (ox_index / tw).min(settings.tiles_x as usize - 1);
            let u = ox_index - tx * tw;
            let symmetry = if settings.variation.flip_and_rotate {
                hash_u64(tx as i32, ty as i32, seed.0) as u8 & 7
            } else {
                0
            };
            let (su, sv) = inverse_symmetry(u, v, tw, th, symmetry, square);
            let z = tile.sample_bilinear(su, sv);
            let mut value = z;
            if settings.mode == WorldMode::Tiled {
                value = tile_center + (value - tile_center) * settings.variation.scale;
                if settings.variation.height_offset_m != 0.0 {
                    let jitter = hash_u64(tx as i32 + 17, ty as i32 - 9, seed.0);
                    let unit = (jitter & 0xffff) as f32 / 65_535.0 - 0.5;
                    value += settings.variation.height_offset_m * unit;
                }
            }
            if settings.macro_variation_m > 0.0 {
                // Continuous across the whole composed world: the same field
                // is evaluated at every output point, so no new seam.
                let wx = ox + ox_index as f64 * sx;
                let wy = oy + oy_index as f64 * sy;
                let n = value_noise(
                    (wx / f64::from(settings.macro_scale_m)) as f32,
                    (wy / f64::from(settings.macro_scale_m)) as f32,
                    seed.0,
                );
                value += (n - 0.5) * 2.0 * settings.macro_variation_m;
            }
            heights[oy_index * ow + ox_index] = value;
        }
    }
    let mut out = HeightField::new(ow, oh, sx, sy, heights).map_err(|e| e.to_string())?;
    out.set_origin(ox, oy);
    Ok(out)
}

/// Make the four edges of a tile match. The interior is modified only inside
/// `blend_cells` samples of a border.
pub fn make_seamless(source: &HeightField, blend_cells: usize) -> Result<HeightField, String> {
    let w = source.width();
    let h = source.height();
    let band = blend_cells.clamp(2, (w.min(h) / 3).max(2));
    let mut heights = source.heights().to_vec();

    // Left/right: ramp from -0.5 at x=0 to +0.5 at x=w-1.
    for y in 0..h {
        let delta = heights[y * w] - heights[y * w + w - 1];
        if delta.abs() <= 1e-6 {
            continue;
        }
        for x in 0..w {
            heights[y * w + x] += edge_ramp(x, w, band) * delta;
        }
    }
    // Top/bottom, after the horizontal correction. The vertical correction is
    // identical on both sides because the left/right edges now match, so it
    // does not reintroduce the horizontal seam.
    for x in 0..w {
        let delta = heights[x] - heights[(h - 1) * w + x];
        if delta.abs() <= 1e-6 {
            continue;
        }
        for y in 0..h {
            heights[y * w + x] += edge_ramp(y, h, band) * delta;
        }
    }
    let (sx, sy) = source.spacing();
    let (ox, oy) = source.origin();
    let mut out = HeightField::new(w, h, sx, sy, heights).map_err(|e| e.to_string())?;
    out.set_origin(ox, oy);
    if let Some(mask) = source.mask() {
        out.set_mask(Some(mask.to_vec()))
            .map_err(|e: HeightFieldError| e.to_string())?;
    }
    Ok(out)
}

/// -0.5 at the first sample, +0.5 at the last, zero in the middle. The smooth
/// ramp keeps the correction continuous and its first derivative zero at the
/// outer edge.
fn edge_ramp(index: usize, size: usize, band: usize) -> f32 {
    if index < band {
        -0.5 * (1.0 - smoothstep(0.0, band as f32, index as f32))
    } else if index + band >= size {
        let d = size - 1 - index;
        0.5 * (1.0 - smoothstep(0.0, band as f32, d as f32))
    } else {
        0.0
    }
}

fn value_noise(x: f32, y: f32, seed: u64) -> f32 {
    let xi = x.floor();
    let yi = y.floor();
    let xf = x - xi;
    let yf = y - yi;
    let ix = xi as i32;
    let iy = yi as i32;
    let v00 = hash_u64(ix, iy, seed) as f32 / u64::MAX as f32;
    let v10 = hash_u64(ix + 1, iy, seed) as f32 / u64::MAX as f32;
    let v01 = hash_u64(ix, iy + 1, seed) as f32 / u64::MAX as f32;
    let v11 = hash_u64(ix + 1, iy + 1, seed) as f32 / u64::MAX as f32;
    let u = fade(xf);
    let v = fade(yf);
    let a = v00 + (v10 - v00) * u;
    let b = v01 + (v11 - v01) * u;
    a + (b - a) * v
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

fn inverse_symmetry(
    u: usize,
    v: usize,
    tw: usize,
    th: usize,
    symmetry: u8,
    square: bool,
) -> (f64, f64) {
    let mut x = u as f64;
    let mut y = v as f64;
    if square {
        let n = tw as f64;
        match symmetry & 3 {
            1 => {
                x = v as f64;
                y = n - u as f64;
            }
            2 => {
                x = n - u as f64;
                y = n - v as f64;
            }
            3 => {
                x = n - v as f64;
                y = u as f64;
            }
            _ => {}
        }
    }
    if symmetry & 4 != 0 {
        x = tw as f64 - x;
    }
    if symmetry & 8 != 0 {
        y = th as f64 - y;
    }
    (x.clamp(0.0, tw as f64), y.clamp(0.0, th as f64))
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if (edge1 - edge0).abs() < f32::EPSILON {
        return if x >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn hash_u64(x: i32, y: i32, seed: u64) -> u64 {
    let mut h = seed;
    h ^= u64::from(x as u32).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    h ^= u64::from(y as u32).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^ (h >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noisy() -> HeightField {
        let size = 65;
        let mut heights = vec![0.0f32; size * size];
        for y in 0..size {
            for x in 0..size {
                heights[y * size + x] = (x as f32 * 1.7).sin() * 80.0
                    + (y as f32 * 2.3).cos() * 45.0
                    + x as f32 * 2.0
                    + y as f32 * 0.5;
            }
        }
        HeightField::new(size, size, 10.0, 10.0, heights).unwrap()
    }

    #[test]
    fn seamless_edges_match() {
        let hf = noisy();
        let tile = make_seamless(&hf, 12).unwrap();
        for y in 0..tile.height() {
            assert!((tile.get(0, y) - tile.get(tile.width() - 1, y)).abs() < 0.01);
        }
        for x in 0..tile.width() {
            assert!((tile.get(x, 0) - tile.get(x, tile.height() - 1)).abs() < 0.01);
        }
        // The correction is localised: the centre is untouched.
        assert!((tile.get(32, 32) - hf.get(32, 32)).abs() < 1e-4);
    }

    #[test]
    fn compose_tiles_and_shares_edges() {
        let hf = noisy();
        let settings = WorldSettings {
            mode: WorldMode::Seamless,
            tiles_x: 2,
            tiles_y: 2,
            blend_cells: 12,
            variation: vf_core::TileVariation {
                flip_and_rotate: false,
                ..vf_core::TileVariation::default()
            },
            ..WorldSettings::default()
        };
        let world = compose(&hf, &settings, Seed::new(3)).unwrap();
        assert_eq!(world.width(), 129);
        assert_eq!(world.height(), 129);
        let tw = 64;
        let tile = make_seamless(&hf, 12).unwrap();
        for y in 0..tile.height() {
            // The shared column x=tw is tile 0's right edge and tile 1's
            // left edge. It must equal both source edges.
            assert!((world.get(tw, y) - tile.get(0, y)).abs() < 0.01);
            assert!((world.get(tw, y) - tile.get(tw, y)).abs() < 0.01);
        }
        // Two independent composes are identical.
        let again = compose(&hf, &settings, Seed::new(3)).unwrap();
        assert_eq!(world.heights(), again.heights());
    }

    #[test]
    fn single_mode_is_identity() {
        let hf = noisy();
        let out = compose(&hf, &WorldSettings::default(), Seed::new(1)).unwrap();
        assert_eq!(out.heights(), hf.heights());
    }
}
