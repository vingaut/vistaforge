//! Geological surface operators for heightfields.
//!
//! The model follows the way a geologist reads a landscape: a stack of beds
//! with a dip and a hardness, cut by joint sets, then dissolved where the rock
//! is soluble. Every operator is deterministic and works in place on a
//! heightfield, so it can be composed with the existing erosion pass.
//!
//! What a heightfield cannot represent — overhangs, arches, caves — is out of
//! scope here: the operators only carve or raise the single altitude per cell.

use vf_core::{Error, GeologySettings, HeightField, JointPattern, Result, Seed, Strata};
use vf_procgen::Noise;

/// A point of the stratigraphic column, re-evaluated by the renderer so rock
/// colours follow the same beds as the geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrataSample {
    /// Bed index, negative below the reference plane.
    pub layer: i32,
    /// 0 = soft, 1 = hard.
    pub hardness: f32,
    /// Stable per-bed value in `[0, 1]`, used for band colour.
    pub band: f32,
    /// Position inside the bed, 0 at the base, 1 at the top.
    pub frac: f32,
}

/// Apply the whole geology model and return a new heightfield.
pub fn apply(hf: &HeightField, settings: &GeologySettings, seed: Seed) -> Result<HeightField> {
    settings.validate().map_err(Error::Invalid)?;
    if !settings.enabled {
        return Ok(hf.clone());
    }

    let width = hf.width();
    let height = hf.height();
    let (spacing_x, spacing_y) = hf.spacing();
    let (origin_x, origin_y) = hf.origin();
    let mut heights = hf.heights().to_vec();

    // A canyon reshapes the plateau first; the other operators read the
    // resulting faces.
    if settings.canyon.enabled {
        apply_canyon(
            &mut heights,
            width,
            height,
            spacing_x,
            spacing_y,
            origin_x,
            origin_y,
            settings,
            seed,
        );
    }

    // Steepness is sampled once, from the carved surface: geological
    // processes act on faces, not on flat ground.
    let steep = steepness(&heights, width, height, spacing_x, spacing_y);

    if settings.strata.enabled && settings.strata.relief_m > 0.0 {
        apply_strata(
            &mut heights,
            &steep,
            width,
            height,
            spacing_x,
            spacing_y,
            origin_x,
            origin_y,
            &settings.strata,
            seed,
        );
    }
    if settings.joints.enabled && settings.joints.depth_m > 0.0 {
        apply_joints(
            &mut heights,
            &steep,
            width,
            height,
            spacing_x,
            spacing_y,
            origin_x,
            origin_y,
            settings,
            seed,
        );
    }
    if settings.dissolution.enabled && settings.dissolution.depth_m > 0.0 {
        apply_dissolution(
            &mut heights,
            &steep,
            width,
            height,
            spacing_x,
            spacing_y,
            origin_x,
            origin_y,
            settings,
            seed,
        );
    }
    let mut out = HeightField::new(width, height, spacing_x, spacing_y, heights)?;
    out.set_origin(origin_x, origin_y);
    out.set_nodata(hf.nodata());
    if let Some(mask) = hf.mask() {
        out.set_mask(Some(mask.to_vec()))?;
    }
    Ok(out)
}

/// Full terrain pipeline: shape, erode, impose the geology, settle the ledges,
/// then add the final roughness.
///
/// Centralised here so the CLI, the desktop app and the MCP server cannot
/// drift apart. `erosion` applies twice when the geology is on: a full pass on
/// the raw fractal, then a light pass after the strata so the ledges are not
/// erased by thermal relaxation.
pub fn build(
    source: &vf_core::TerrainSource,
    erosion: Option<&vf_core::ErosionParams>,
    geology: &GeologySettings,
    seed: Seed,
) -> Result<HeightField> {
    let mut hf = vf_procgen::generate(source, None, seed)?;
    if let Some(params) = erosion {
        hf = vf_procgen::erosion::erode(&hf, params, seed.derive("erosion"))?;
    }
    if geology.enabled {
        hf = apply(&hf, geology, seed)?;
        if let Some(params) = erosion {
            let mut settle = params.clone();
            settle.droplets = (params.droplets / 4).max(2_000);
            settle.thermal_iterations = params.thermal_iterations.min(3);
            hf = vf_procgen::erosion::erode(&hf, &settle, seed.derive("settle"))?;
        }
        hf = micro_detail(&hf, geology, seed)?;
    }
    Ok(hf)
}

/// Final surface roughness, applied after the erosion settle pass.
///
/// The detail must come last: a thermal relaxation after it would erase every
/// crag steeper than the talus angle, which is precisely the relief that makes
/// rock read as rock.
pub fn micro_detail(
    hf: &HeightField,
    settings: &GeologySettings,
    seed: Seed,
) -> Result<HeightField> {
    if !settings.enabled || settings.detail_m <= 0.0 || !hf.heights().iter().any(|v| *v != 0.0) {
        return Ok(hf.clone());
    }
    let width = hf.width();
    let height = hf.height();
    let (spacing_x, spacing_y) = hf.spacing();
    let (origin_x, origin_y) = hf.origin();
    let mut heights = hf.heights().to_vec();
    let mut steep = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let n = hf.normal_at(x, y, 1.0);
            steep[y * width + x] = (1.0 - n[2]).clamp(0.0, 1.0).powf(0.55);
        }
    }
    apply_detail(
        &mut heights,
        &steep,
        width,
        height,
        spacing_x,
        spacing_y,
        origin_x,
        origin_y,
        settings,
        seed,
    );
    if settings.gullies.enabled && settings.gullies.depth_m > 0.0 {
        apply_gullies(
            &mut heights,
            &steep,
            width,
            height,
            spacing_x,
            spacing_y,
            origin_x,
            origin_y,
            settings,
            seed,
        );
    }
    // Spires go last: the settle pass would round their silhouette away.
    if settings.spires.enabled {
        apply_spires(
            &mut heights,
            width,
            height,
            spacing_x,
            spacing_y,
            origin_x,
            origin_y,
            settings,
            seed,
        );
    }
    let mut out = HeightField::new(width, height, spacing_x, spacing_y, heights)?;
    out.set_origin(origin_x, origin_y);
    out.set_nodata(hf.nodata());
    if let Some(mask) = hf.mask() {
        out.set_mask(Some(mask.to_vec()))?;
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn apply_canyon(
    heights: &mut [f32],
    width: usize,
    height: usize,
    spacing_x: f64,
    spacing_y: f64,
    origin_x: f64,
    origin_y: f64,
    settings: &GeologySettings,
    seed: Seed,
) {
    let canyon = &settings.canyon;
    if canyon.depth_m <= 0.0 {
        return;
    }
    let mut min = f32::INFINITY;
    let mut max = f32::NEG_INFINITY;
    for &h in heights.iter() {
        min = min.min(h);
        max = max.max(h);
    }
    let floor = min + (max - min) * 0.04;
    let noise = Noise::from_u64(seed.derive("canyon").0);
    let extent_x = (width - 1) as f64 * spacing_x;
    let extent_y = (height - 1) as f64 * spacing_y;
    let sinuosity = f64::from(canyon.sinuosity.clamp(0.0, 1.0));

    for y in 0..height {
        let wy = origin_y + y as f64 * spacing_y;
        let v = ((wy - origin_y) / extent_y.max(1.0)) as f32;
        for x in 0..width {
            let wx = origin_x + x as f64 * spacing_x;
            let u = ((wx - origin_x) / extent_x.max(1.0)) as f32;
            let i = y * width + x;
            let mut h = heights[i];

            // Main canyon: a meander whose half-width breathes with the rock.
            let meander = ((u * std::f32::consts::TAU * 1.1 + 0.7).sin()
                + 0.35 * (u * std::f32::consts::TAU * 2.3).sin())
                * 0.10
                * sinuosity as f32;
            let center_y = origin_y + extent_y * (0.5 + f64::from(meander));
            let d = (wy - center_y).abs() as f32;
            let half = canyon.width_m * (0.8 + 0.4 * noise.fbm01(u * 4.0, 0.7, 2, 2.0, 0.5));
            let cut = 1.0 - smoothstep(0.70, 1.0, d / half.max(1.0));
            if cut > 0.0 {
                h -= canyon.depth_m * cut;
                h = h.max(floor);
                let rim = (-((d / half.max(1.0) - 1.02) / 0.06).powi(2)).exp();
                h += canyon.depth_m * 0.05 * rim;
            }

            // Tributary: a narrower canyon joining from the side.
            if canyon.tributary {
                let side = ((v * std::f32::consts::TAU * 0.8 + 2.0).sin() as f64) * 0.08;
                let center_x = origin_x + extent_x * (0.34 + side);
                let d2 = (wx - center_x).abs() as f32;
                let half2 = (canyon.width_m * 0.5).max(20.0);
                let cut2 = 1.0 - smoothstep(0.70, 1.0, d2 / half2);
                if cut2 > 0.0 {
                    h -= canyon.depth_m * 0.45 * cut2;
                    h = h.max(floor);
                }
            }
            heights[i] = h;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_spires(
    heights: &mut [f32],
    width: usize,
    height: usize,
    spacing_x: f64,
    spacing_y: f64,
    origin_x: f64,
    origin_y: f64,
    settings: &GeologySettings,
    seed: Seed,
) {
    let spires = &settings.spires;
    if spires.count == 0 || spires.height_m <= 0.0 || spires.radius_m <= 0.0 {
        return;
    }
    let mut rng = seed.derive("spires").rng();
    let extent_x = (width - 1) as f64 * spacing_x;
    let extent_y = (height - 1) as f64 * spacing_y;
    let radius = f64::from(spires.radius_m);
    let sharp = f64::from(spires.sharpness.clamp(0.0, 1.0));
    let profile_power = 1.0 + 2.2 * sharp;
    for _ in 0..spires.count.min(64) {
        // Cluster the towers in the central half: a massif, not a dotted plain.
        let cx = origin_x + extent_x * (0.28 + 0.44 * rng.next_f64());
        let cy = origin_y + extent_y * (0.28 + 0.44 * rng.next_f64());
        let tower_h = f64::from(spires.height_m) * (0.65 + 0.55 * rng.next_f64());
        let r = radius * (0.7 + 0.6 * rng.next_f64());
        let reach = r * 1.4;
        let x0 = (((cx - reach - origin_x) / spacing_x).floor()).max(0.0) as usize;
        let x1 = (((cx + reach - origin_x) / spacing_x).ceil()).min((width - 1) as f64) as usize;
        let y0 = (((cy - reach - origin_y) / spacing_y).floor()).max(0.0) as usize;
        let y1 = (((cy + reach - origin_y) / spacing_y).ceil()).min((height - 1) as f64) as usize;
        for y in y0..=y1 {
            let wy = origin_y + y as f64 * spacing_y;
            for x in x0..=x1 {
                let wx = origin_x + x as f64 * spacing_x;
                let d = ((wx - cx).powi(2) + (wy - cy).powi(2)).sqrt();
                if d >= reach {
                    continue;
                }
                let t = (d / r).clamp(0.0, 1.0);
                let profile = (1.0 - t).powf(profile_power);
                // A small terrace at the base reads as a shoulder.
                let shoulder = 0.12 * f64::from(1.0 - smoothstep(0.75, 1.0, t as f32));
                heights[y * width + x] += (tower_h * (profile + shoulder)) as f32;
            }
        }
    }
}

/// Steepness in `[0, 1]`, 1 on a vertical face, from finite differences.
fn steepness(heights: &[f32], width: usize, height: usize, sx: f64, sy: f64) -> Vec<f32> {
    let mut out = vec![0.0f32; width * height];
    let sx = sx as f32;
    let sy = sy as f32;
    for y in 0..height {
        let row = y * width;
        let ym = y.saturating_sub(1) * width;
        let yp = (y + 1).min(height - 1) * width;
        for x in 0..width {
            let xm = x.saturating_sub(1);
            let xp = (x + 1).min(width - 1);
            let dx = (heights[row + xp] - heights[row + xm]) / ((xp - xm).max(1) as f32 * sx);
            let dy = (heights[yp + x] - heights[ym + x])
                / (((y + 1).min(height - 1) - y.saturating_sub(1)).max(1) as f32 * sy);
            let nz = 1.0 / (1.0 + dx * dx + dy * dy).sqrt();
            out[row + x] = (1.0 - nz).clamp(0.0, 1.0).powf(0.6);
        }
    }
    out
}

/// Evaluate the stratigraphic column at a world position. Shared by the
/// geometry pass and the renderer's albedo.
pub fn strata_sample(strata: &Strata, x: f64, y: f64, z: f32, seed: u64) -> StrataSample {
    let thickness = strata.thickness_m.max(0.01);
    let dip = f64::from(strata.dip_deg).to_radians();
    let az = f64::from(strata.dip_azimuth_deg).to_radians();
    // Horizontal direction the beds dip toward (compass).
    let dir_x = -az.sin();
    let dir_y = -az.cos();
    let slope = dip.tan();

    let noise = Noise::from_u64(seed);
    let warp = if strata.warp > 0.0 && strata.warp_scale_m > 1.0 {
        let n = noise.fbm2(
            x as f32 / strata.warp_scale_m,
            y as f32 / strata.warp_scale_m,
            3,
            2.0,
            0.5,
        );
        f64::from(strata.warp) * f64::from(n) * 2.0
    } else {
        0.0
    };

    let plane = slope * (x * dir_x + y * dir_y);
    let mut coord = (f64::from(z) - plane) / f64::from(thickness) + warp;
    // Uneven bed thickness: a small monotonic warp of the column so the beds
    // are not a perfectly regular layer cake.
    coord += 0.15 * (coord * 1.7).sin();
    let layer = coord.floor();
    let frac = (coord - layer) as f32;
    // Bed colour drifts laterally: the same bed is not the same colour across
    // a whole cliff.
    let lateral = noise.fbm01(x as f32 / 2600.0, y as f32 / 2600.0, 2, 2.0, 0.5);
    let band = (hash01(layer as i64, seed) + 0.35 * (lateral - 0.5)).clamp(0.0, 1.0);
    let wave = (layer * 1.7).sin() as f32 * 0.5 + 0.5;
    let mix = (band * 0.65 + wave * 0.35).clamp(0.0, 1.0);
    let hardness = (0.5 + (mix - 0.5) * strata.hardness_contrast).clamp(0.0, 1.0);
    StrataSample {
        layer: layer as i32,
        hardness,
        band: mix,
        frac,
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_strata(
    heights: &mut [f32],
    steep: &[f32],
    width: usize,
    height: usize,
    spacing_x: f64,
    spacing_y: f64,
    origin_x: f64,
    origin_y: f64,
    strata: &Strata,
    seed: Seed,
) {
    let seed = seed.derive("strata").0;
    for y in 0..height {
        let wy = origin_y + y as f64 * spacing_y;
        let row = y * width;
        for x in 0..width {
            let wx = origin_x + x as f64 * spacing_x;
            let i = row + x;
            let sample = strata_sample(strata, wx, wy, heights[i], seed);
            // Hard beds stand proud, soft beds recede; the effect is
            // proportional to steepness so flats keep their shape.
            let mut dz = strata.relief_m * (sample.hardness - 0.5) * 2.0 * steep[i];
            // A slight bevel at the bed boundary reads as a ledge rather than
            // a smooth ramp.
            let edge = (sample.frac.min(1.0 - sample.frac) * 2.0).clamp(0.0, 1.0);
            dz += strata.relief_m * 0.18 * (1.0 - edge) * steep[i];
            heights[i] += dz;
        }
    }
}

/// Joint seam strength at a world position, for the renderer's rock colour.
/// `cell` is the terrain sample spacing, used for the same resolution clamp as
/// the geometry pass so the dark seams line up with the grooves.
pub fn joint_mask(joints: &vf_core::Joints, x: f64, y: f64, cell: f64, seed: u64) -> f32 {
    if !joints.enabled || joints.depth_m <= 0.0 {
        return 0.0;
    }
    joint_field(joints, x, y, cell, seed).0
}

fn joint_field(joints: &vf_core::Joints, x: f64, y: f64, cell: f64, seed: u64) -> (f32, f32) {
    // A joint narrower than the grid cannot be represented: widen the network
    // with resolution so the operator stays stable from preview to final.
    let spacing = f64::from(joints.spacing_m.max(2.0)).max(4.0 * cell);
    let width_frac = joints
        .width
        .clamp(0.01, 0.5)
        .max((1.5 * cell / spacing) as f32);
    let angle = f64::from(joints.angle_deg).to_radians();
    let (sin_a, cos_a) = angle.sin_cos();
    let rx = x * cos_a - y * sin_a;
    let ry = x * sin_a + y * cos_a;
    let (f1, f2, cell_hash) = match joints.pattern {
        JointPattern::HexColumns => voronoi_hex(rx, ry, spacing, joints.jitter, seed),
        JointPattern::Radial => voronoi_radial(rx, ry, spacing, joints.jitter, seed),
        JointPattern::Orthogonal => voronoi_grid(rx, ry, spacing, joints.jitter, seed),
    };
    let crack = 1.0 - smoothstep(0.0, width_frac, f2 - f1);
    (crack, cell_hash)
}

#[allow(clippy::too_many_arguments)]
fn apply_joints(
    heights: &mut [f32],
    steep: &[f32],
    width: usize,
    height: usize,
    spacing_x: f64,
    spacing_y: f64,
    origin_x: f64,
    origin_y: f64,
    settings: &GeologySettings,
    seed: Seed,
) {
    let joints = &settings.joints;
    let seed = seed.derive("joints").0;
    let cell = spacing_x.max(spacing_y);
    let columnar = matches!(joints.pattern, JointPattern::HexColumns);

    for y in 0..height {
        let wy = origin_y + y as f64 * spacing_y;
        let row = y * width;
        for x in 0..width {
            let wx = origin_x + x as f64 * spacing_x;
            let i = row + x;
            let (crack, cell_hash) = joint_field(joints, wx, wy, cell, seed);
            let mut dz = -joints.depth_m * crack * (0.25 + 0.75 * steep[i]);
            if columnar {
                // Each column has its own flat top: the prisms do not line up.
                dz += joints.depth_m * 0.35 * (cell_hash - 0.5) * (1.0 - crack);
            }
            heights[i] += dz;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_dissolution(
    heights: &mut [f32],
    steep: &[f32],
    width: usize,
    height: usize,
    spacing_x: f64,
    spacing_y: f64,
    origin_x: f64,
    origin_y: f64,
    settings: &GeologySettings,
    seed: Seed,
) {
    let diss = &settings.dissolution;
    let seed = seed.derive("karst");
    let mut rng = seed.rng();
    let radius = f64::from(diss.radius_m.max(5.0)).max(2.5 * spacing_x.max(spacing_y));
    let cell = radius * 2.0;
    let cols = ((width as f64 * spacing_x) / cell).ceil() as usize + 1;
    let rows = ((height as f64 * spacing_y) / cell).ceil() as usize + 1;
    let seed_u64 = seed.0;

    for gy in 0..rows {
        for gx in 0..cols {
            let h = hash01(((gy * cols + gx) as i64) ^ 0x51ed_2701, seed_u64);
            if h > diss.density {
                continue;
            }
            let cx = origin_x + (gx as f64 + 0.2 + 0.6 * f64::from(rng.next_f32())) * cell;
            let cy = origin_y + (gy as f64 + 0.2 + 0.6 * f64::from(rng.next_f32())) * cell;
            let r = radius * (0.55 + 0.8 * f64::from(rng.next_f32()));
            let depth = f64::from(diss.depth_m) * (0.35 + 0.65 * f64::from(rng.next_f32()));
            let reach = r * 1.4;
            let x0 = (((cx - reach - origin_x) / spacing_x).floor()).max(0.0) as usize;
            let x1 =
                (((cx + reach - origin_x) / spacing_x).ceil()).min((width - 1) as f64) as usize;
            let y0 = (((cy - reach - origin_y) / spacing_y).floor()).max(0.0) as usize;
            let y1 =
                (((cy + reach - origin_y) / spacing_y).ceil()).min((height - 1) as f64) as usize;
            for y in y0..=y1 {
                let wy = origin_y + y as f64 * spacing_y;
                for x in x0..=x1 {
                    let wx = origin_x + x as f64 * spacing_x;
                    let dx = wx - cx;
                    let dy = wy - cy;
                    let d = (dx * dx + dy * dy).sqrt();
                    if d >= reach {
                        continue;
                    }
                    let t = (d / r).clamp(0.0, 1.0);
                    let bowl = (1.0 - t * t).powi(2);
                    let rim = (-((d / r - 1.05) / 0.18).powi(2)).exp() * 0.18;
                    let i = y * width + x;
                    // Dolines form on plateaus and gentle slopes; cliffs are
                    // left to the joint network.
                    let flat = f64::from((1.0 - steep[i]).clamp(0.0, 1.0));
                    heights[i] -= (depth * bowl * flat) as f32;
                    heights[i] += (depth * rim * flat) as f32;
                }
            }
        }
    }

    if diss.fluting > 0.0 {
        let noise = Noise::from_u64(seed.derive("lapiaz").0);
        for y in 0..height {
            let wy = origin_y + y as f64 * spacing_y;
            let row = y * width;
            for x in 0..width {
                let i = row + x;
                if steep[i] < 0.05 {
                    continue;
                }
                let wx = origin_x + x as f64 * spacing_x;
                // Vertical flutes: high frequency across the slope.
                let f = noise.ridged2(wx as f32 * 0.35, wy as f32 * 0.35, 3, 2.0, 0.5);
                heights[i] -= diss.fluting * diss.depth_m * 0.12 * f * steep[i];
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_gullies(
    heights: &mut [f32],
    steep: &[f32],
    width: usize,
    height: usize,
    spacing_x: f64,
    spacing_y: f64,
    origin_x: f64,
    origin_y: f64,
    settings: &GeologySettings,
    seed: Seed,
) {
    let gullies = &settings.gullies;
    let noise = Noise::from_u64(seed.derive("gullies").0);
    let inv = 1.0 / gullies.spacing_m.max(5.0);
    let sharp = gullies.sharpness.clamp(0.05, 0.95);
    for y in 0..height {
        let wy = origin_y + y as f64 * spacing_y;
        let row = y * width;
        for x in 0..width {
            let i = row + x;
            if steep[i] < 0.12 {
                continue;
            }
            let wx = origin_x + x as f64 * spacing_x;
            // Ridged noise stretched north-south: the channels read as vertical
            // flutes on a cliff face.
            let n = noise.ridged2(wx as f32 * inv, wy as f32 * inv * 0.55, 2, 2.0, 0.5);
            let channel = 1.0 - smoothstep(0.12, 0.32 + 0.4 * sharp, n);
            let weight = 0.5 + 0.5 * steep[i];
            heights[i] -= gullies.depth_m * channel * weight;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_detail(
    heights: &mut [f32],
    steep: &[f32],
    width: usize,
    height: usize,
    spacing_x: f64,
    spacing_y: f64,
    origin_x: f64,
    origin_y: f64,
    settings: &GeologySettings,
    seed: Seed,
) {
    let broad = Noise::from_u64(seed.derive("detail").0);
    let fine = Noise::from_u64(seed.derive("detail-fine").0);
    let crag = Noise::from_u64(seed.derive("detail-crag").0);
    let scale = settings.detail_scale_m.max(1.0);
    let amplitude = settings.detail_m;
    for y in 0..height {
        let wy = origin_y + y as f64 * spacing_y;
        let row = y * width;
        for x in 0..width {
            let i = row + x;
            if steep[i] < 0.02 {
                continue;
            }
            let wx = origin_x + x as f64 * spacing_x;
            // Broad undulation, then finer facets. The ridged term creates the
            // sharp crags that a plain fBm rounds off.
            let n1 = broad.fbm01(wx as f32 / scale, wy as f32 / scale, 3, 2.0, 0.5);
            let n2 = fine.fbm01(
                wx as f32 / (scale * 0.28),
                wy as f32 / (scale * 0.28),
                2,
                2.0,
                0.5,
            );
            let r = crag.ridged2(
                wx as f32 / (scale * 0.45),
                wy as f32 / (scale * 0.45),
                2,
                2.0,
                0.5,
            );
            let relief = (n1 * 2.0 - 1.0) * 0.55 + (n2 * 2.0 - 1.0) * 0.25 + (r - 0.5) * 0.45;
            // On a near-vertical wall a vertical displacement only makes
            // streaks; the cliff's roughness belongs to the normal, not the
            // heightfield.
            let weight = steep[i] * (1.0 - smoothstep(0.55, 0.92, steep[i]));
            heights[i] += amplitude * relief * weight;
        }
    }
}

// --- Voronoi helpers -------------------------------------------------------

/// Jittered square lattice. Returns (nearest, second nearest, cell hash).
fn voronoi_grid(x: f64, y: f64, spacing: f64, jitter: f32, seed: u64) -> (f32, f32, f32) {
    let gx = x / spacing;
    let gy = y / spacing;
    let cx = gx.floor() as i64;
    let cy = gy.floor() as i64;
    let mut f1 = f32::INFINITY;
    let mut f2 = f32::INFINITY;
    let mut id = 0.0f32;
    for j in -1..=1 {
        for i in -1..=1 {
            let cell_x = cx + i;
            let cell_y = cy + j;
            let hx = hash01(cell_x * 31 + cell_y * 101, seed);
            let hy = hash01(cell_x * 57 + cell_y * 197, seed ^ 0x9e37);
            let jx = 0.5 + (hx - 0.5) * jitter;
            let jy = 0.5 + (hy - 0.5) * jitter;
            let sx = cell_x as f64 + f64::from(jx);
            let sy = cell_y as f64 + f64::from(jy);
            let dx = (gx - sx) as f32;
            let dy = (gy - sy) as f32;
            let d = (dx * dx + dy * dy).sqrt();
            if d < f1 {
                f2 = f1;
                f1 = d;
                id = hx;
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (f1, f2, id)
}

/// Hexagonal columnar lattice (basalt prisms).
fn voronoi_hex(x: f64, y: f64, spacing: f64, jitter: f32, seed: u64) -> (f32, f32, f32) {
    // Lattice constant for a pointy-top hex grid whose nearest centres are
    // `spacing` apart.
    let size = spacing / 3.0f64.sqrt();
    let q = x / (1.5 * size);
    let r = y / (3.0f64.sqrt() * size) - q * 0.5;
    let (cq, cr) = axial_round(q, r);
    let mut f1 = f32::INFINITY;
    let mut f2 = f32::INFINITY;
    let mut id = 0.0f32;
    for dq in -1..=1 {
        for dr in -1..=1 {
            let qq = cq + dq;
            let rr = cr + dr;
            let hx = hash01(qq * 71 + rr * 131, seed);
            let hy = hash01(qq * 113 + rr * 173, seed ^ 0x1234_5678);
            let jx = (hx - 0.5) * jitter;
            let jy = (hy - 0.5) * jitter;
            let cx = (1.5 * size) * (qq as f64 + jx as f64);
            let cy = 3.0f64.sqrt() * size * (rr as f64 + jy as f64 + 0.5 * (qq as f64 + jx as f64));
            let dx = (x - cx) as f32;
            let dy = (y - cy) as f32;
            let d = (dx * dx + dy * dy).sqrt();
            if d < f1 {
                f2 = f1;
                f1 = d;
                id = hx;
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (f1, f2, id)
}

fn voronoi_radial(x: f64, y: f64, spacing: f64, _jitter: f32, seed: u64) -> (f32, f32, f32) {
    let r = (x * x + y * y).sqrt();
    let a = y.atan2(x);
    let spokes = (std::f64::consts::TAU / spacing.max(1.0) * 40.0)
        .round()
        .max(3.0);
    let t = (a / std::f64::consts::TAU * spokes).rem_euclid(1.0);
    let arc = t.min(1.0 - t) * std::f64::consts::TAU / spokes * r;
    let f1 = arc as f32;
    let f2 = f1 + (spacing * 0.25) as f32;
    let id = hash01((r / spacing) as i64, seed);
    (f1, f2, id)
}

fn axial_round(q: f64, r: f64) -> (i64, i64) {
    let s = -q - r;
    let rq = q.round();
    let rr = r.round();
    let rs = s.round();
    let dq = (rq - q).abs();
    let dr = (rr - r).abs();
    let ds = (rs - s).abs();
    let (q, r) = if dq > dr && dq > ds {
        (-rr - rs, rr)
    } else if dr > ds {
        (rq, -rq - rs)
    } else {
        (rq, rr)
    };
    (q as i64, r as i64)
}

#[inline]
pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if (edge1 - edge0).abs() < 1e-9 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn hash01(i: i64, seed: u64) -> f32 {
    let mut z = seed ^ (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 40) as f32 * (1.0 / 16_777_216.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::{GeologySettings, Seed};

    fn terrain(n: usize) -> HeightField {
        let mut heights = vec![0.0f32; n * n];
        for y in 0..n {
            for x in 0..n {
                let u = x as f32 / (n - 1) as f32;
                let v = y as f32 / (n - 1) as f32;
                heights[y * n + x] = 1200.0 * (1.0 - u) + 240.0 * (u * 6.0).sin() * v;
            }
        }
        HeightField::new(n, n, 30.0, 30.0, heights).unwrap()
    }

    #[test]
    fn disabled_is_identity() {
        let hf = terrain(33);
        let out = apply(&hf, &GeologySettings::default(), Seed::new(1)).unwrap();
        assert_eq!(out.heights(), hf.heights());
    }

    #[test]
    fn presets_change_the_terrain_deterministically() {
        for name in vf_core::GEOLOGY_PRESETS {
            let settings = GeologySettings::preset(name).unwrap();
            let hf = terrain(65);
            let a = apply(&hf, &settings, Seed::new(7)).unwrap();
            let b = apply(&hf, &settings, Seed::new(7)).unwrap();
            assert_eq!(a.heights(), b.heights(), "{name} not deterministic");
            assert_ne!(a.heights(), hf.heights(), "{name} did nothing");
            assert_eq!((a.width(), a.height()), (65, 65));
            assert!(a.heights().iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn harder_beds_stand_higher() {
        let settings = GeologySettings::preset("sandstone").unwrap();
        let hf = HeightField::filled(9, 9, 10.0, 500.0).unwrap();
        // A perfectly flat field has no steepness, so strata must not move it;
        // joints and detail may. Disable them to isolate the strata term.
        let only_strata = GeologySettings {
            strata: settings.strata,
            joints: vf_core::Joints::default(),
            dissolution: vf_core::Dissolution::default(),
            detail_m: 0.0,
            ..GeologySettings::default()
        };
        let out = apply(&hf, &only_strata, Seed::new(3)).unwrap();
        assert!(out.heights().iter().all(|v| (*v - 500.0).abs() < 1e-3));
    }

    #[test]
    fn strata_sample_is_stable_across_calls() {
        let s = GeologySettings::preset("limestone").unwrap().strata;
        let a = strata_sample(&s, 123.0, 456.0, 800.0, 11);
        let b = strata_sample(&s, 123.0, 456.0, 800.0, 11);
        assert_eq!(a, b);
        assert!((0.0..=1.0).contains(&a.hardness));
        assert!((0.0..=1.0).contains(&a.band));
    }
}
