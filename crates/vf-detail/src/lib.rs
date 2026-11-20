//! Continuous rock detail above a heightfield.
//!
//! The sampler is the bridge between the editable 2.5D base and the render /
//! export mesh. It evaluates a resolution-independent relief made of:
//!
//! * a band-limited fractal base,
//! * directional grooves (flutes, runnels),
//! * ridged crests,
//! * a deterministic crater field with rims and ejecta,
//! * exfoliation sheets,
//! * vesicles.
//!
//! Every layer carries a wavelength. When the caller knows the pixel footprint
//! of the point being shaded, octaves and layers smaller than the footprint are
//! faded out instead of being point-sampled: that is the anti-aliasing contract
//! of the module. The same function is used by the CPU renderer and the Blender
//! exporter, so both see the same surface.

use vf_core::{DetailSettings, HeightField, Seed};
use vf_procgen::{Noise, value2};

/// Evaluate the detailed surface for one base heightfield.
#[derive(Debug, Clone)]
pub struct DetailSampler<'a> {
    hf: &'a HeightField,
    settings: &'a DetailSettings,
    base_noise: Noise,
    warp_noise: Noise,
    ridge_noise: Noise,
    groove_noise: Noise,
    crater_noise: Noise,
    exfoliation_noise: Noise,
    vesicle_noise: Noise,
    tafoni_noise: Noise,
    bench_noise: Noise,
    seed: u64,
    origin: (f64, f64),
    spacing: (f64, f64),
}

impl<'a> DetailSampler<'a> {
    pub fn new(hf: &'a HeightField, settings: &'a DetailSettings, project_seed: Seed) -> Self {
        let origin = hf.origin();
        let spacing = hf.spacing();
        let seed = project_seed.derive("detail").0 ^ settings.seed;
        let seed_obj = Seed(seed);
        let derived = |name: &str| Seed(seed_obj.derive(name).0);
        Self {
            hf,
            settings,
            base_noise: Noise::new(derived("base")),
            warp_noise: Noise::new(derived("warp")),
            ridge_noise: Noise::new(derived("ridges")),
            groove_noise: Noise::new(derived("grooves")),
            crater_noise: Noise::new(derived("craters")),
            exfoliation_noise: Noise::new(derived("exfoliation")),
            vesicle_noise: Noise::new(derived("vesicles")),
            tafoni_noise: Noise::new(derived("tafoni")),
            bench_noise: Noise::new(derived("benches")),
            seed,
            origin,
            spacing,
        }
    }

    pub fn settings(&self) -> &DetailSettings {
        self.settings
    }

    pub fn height_field(&self) -> &HeightField {
        self.hf
    }

    fn grid_coords(&self, x: f64, y: f64) -> (f64, f64) {
        (
            (x - self.origin.0) / self.spacing.0,
            (y - self.origin.1) / self.spacing.1,
        )
    }

    /// Base terrain height at world `(x, y)`.
    pub fn base_height(&self, x: f64, y: f64) -> f32 {
        let (gx, gy) = self.grid_coords(x, y);
        self.hf.sample_bilinear(gx, gy)
    }

    /// Detailed absolute height in metres.
    ///
    /// `footprint_m` is the world-space size of one pixel at the sample. Pass
    /// `0.0` to disable band limiting (mesh construction at a known spacing
    /// can pass the vertex spacing instead).
    pub fn height(&self, x: f64, y: f64, footprint_m: f32) -> f32 {
        self.base_height(x, y) + self.detail(x, y, footprint_m)
    }

    /// Height with the detail scaled by `detail_scale` (0 = base surface).
    /// Used by the focus patch to fade its border into the coarse mesh.
    pub fn height_scaled(&self, x: f64, y: f64, footprint_m: f32, detail_scale: f32) -> f32 {
        self.base_height(x, y) + self.detail(x, y, footprint_m) * detail_scale.clamp(0.0, 1.0)
    }

    /// Detailed normal, including the base slope. Finite differences use a
    /// probe distance adapted to the footprint so that a distant, band-limited
    /// surface produces a stable normal.
    pub fn normal(&self, x: f64, y: f64, footprint_m: f32) -> [f32; 3] {
        self.normal_scaled(x, y, footprint_m, 1.0)
    }

    /// Normal of [`Self::height_scaled`].
    pub fn normal_scaled(&self, x: f64, y: f64, footprint_m: f32, detail_scale: f32) -> [f32; 3] {
        let eps = (footprint_m * 2.0).clamp(0.35, 80.0) as f64;
        let hx = self.height_scaled(x + eps, y, footprint_m, detail_scale)
            - self.height_scaled(x - eps, y, footprint_m, detail_scale);
        let hy = self.height_scaled(x, y + eps, footprint_m, detail_scale)
            - self.height_scaled(x, y - eps, footprint_m, detail_scale);
        let dx = hx / (2.0 * eps as f32);
        let dy = hy / (2.0 * eps as f32);
        let inv = 1.0 / (dx * dx + dy * dy + 1.0).sqrt();
        [-dx * inv, -dy * inv, inv]
    }

    /// Detail offset in metres, without the base height.
    ///
    /// Vertical displacement is deliberately attenuated on near-vertical
    /// faces. A heightfield has one height per (x, y): moving a vertex up on a
    /// 80° wall produces a needle, not a groove. The high-frequency story on
    /// cliffs is carried by the shader normal (see `vf-render`), while the
    /// geometry keeps ledges and broad shapes. This is the same rule the
    /// geological `micro_detail` pass learned the hard way.
    pub fn detail(&self, x: f64, y: f64, footprint_m: f32) -> f32 {
        let d = self.settings;
        if !d.enabled || d.strength <= 0.0 || d.amplitude_m <= 0.0 {
            return 0.0;
        }
        let x = x as f32;
        let y = y as f32;
        let footprint = footprint_m.max(0.0);
        let slope = self.slope_mask(x, y);
        let slope_weight = 1.0 - d.slope_bias + d.slope_bias * smoothstep(0.05, 0.65, slope);
        let vertical = 1.0 - 0.94 * smoothstep(0.55, 0.86, slope);

        // Low-frequency heterogeneity: every layer sees a different value of
        // this field, so the bands, plates and flutes stop lining up. The
        // coordinates are also warped before evaluation, which is what breaks
        // the contour regularity of the exfoliation and crater grids.
        let variation = self.variation(x, y);
        let patchiness = 1.0 - 0.55 * d.heterogeneity + 1.10 * d.heterogeneity * variation;
        let (xw, yw) = self.warped(x, y);

        let mut sum = self.fractal(xw, yw, footprint) * slope_weight;
        sum += self.ridges(xw, yw, footprint, variation) * slope_weight;
        sum += self.grooves(xw, yw, footprint, variation);
        sum += self.craters(xw, yw, footprint, variation);
        sum += self.exfoliation(xw, yw, footprint, variation) * slope_weight;
        sum += self.vesicles(xw, yw, footprint, variation);
        sum += self.tafoni(xw, yw, footprint, slope, variation);
        sum += self.benches(xw, yw, footprint, slope, variation);
        sum += self.joints(xw, yw, footprint, variation);
        sum += self.ooliths(xw, yw, footprint, slope, variation);
        (sum * vertical * d.strength * patchiness.clamp(0.35, 1.65)).clamp(-80.0, 80.0)
    }

    /// Large-scale patch field in `[0, 1]`. Independent from the layer noise
    /// so that amplitude changes do not correlate with ridge positions.
    fn variation(&self, x: f32, y: f32) -> f32 {
        let scale = (self.settings.scale_m * 7.0).max(90.0);
        self.warp_noise
            .fbm01(x / scale + 17.0, y / scale - 9.0, 3, 2.0, 0.5)
    }

    /// Domain warp applied to every layer. At zero heterogeneity this is the
    /// identity, so existing projects keep their exact surface.
    fn warped(&self, x: f32, y: f32) -> (f32, f32) {
        let h = self.settings.heterogeneity;
        if h <= 0.0 {
            return (x, y);
        }
        let scale = (self.settings.scale_m * 4.0).max(40.0);
        let amp = h * self.settings.scale_m * 0.55;
        let wx = self.warp_noise.fbm2(x / scale, y / scale, 3, 2.0, 0.5) * amp;
        let wy = self
            .warp_noise
            .fbm2(x / scale + 31.7, y / scale - 13.1, 3, 2.0, 0.5)
            * amp;
        (x + wx, y + wy)
    }

    /// Base slope as `1 - n.z`, evaluated at the heightfield cell scale.
    fn slope_mask(&self, x: f32, y: f32) -> f32 {
        let eps = self.spacing.0.max(self.spacing.1).max(1.0) as f32;
        let hx = self.base_height(f64::from(x + eps), f64::from(y))
            - self.base_height(f64::from(x - eps), f64::from(y));
        let hy = self.base_height(f64::from(x), f64::from(y + eps))
            - self.base_height(f64::from(x), f64::from(y - eps));
        let dx = hx / (2.0 * eps);
        let dy = hy / (2.0 * eps);
        (1.0 - 1.0 / (dx * dx + dy * dy + 1.0).sqrt()).clamp(0.0, 1.0)
    }

    fn fractal(&self, x: f32, y: f32, footprint: f32) -> f32 {
        let d = self.settings;
        // Domain warp: the fractal is evaluated on a gently bent coordinate
        // system, which removes the "cotton wool" look of plain fBm.
        let warp_scale = (d.scale_m * 0.8).max(1.0);
        let wx = self
            .base_noise
            .fbm2(x / warp_scale, y / warp_scale, 3, 2.0, 0.5)
            * d.warp
            * warp_scale
            * 0.38;
        let wy = self
            .base_noise
            .fbm2(x / warp_scale + 11.3, y / warp_scale - 5.7, 3, 2.0, 0.5)
            * d.warp
            * warp_scale
            * 0.38;
        let mut px = (x + wx) / d.scale_m;
        let mut py = (y + wy) / d.scale_m;
        let mut frequency = 1.0f32;
        let mut amplitude = 1.0f32;
        let mut sum = 0.0f32;
        let mut norm = 0.0f32;
        for octave in 0..d.octaves.max(1) {
            let wavelength = d.scale_m / frequency;
            let band = band_limit(wavelength, footprint);
            if band > 0.0 {
                sum += amplitude * band * self.base_noise.noise2(px * frequency, py * frequency);
                norm += amplitude * band;
            }
            amplitude *= d.gain;
            frequency *= d.lacunarity;
            let angle = 0.5 + octave as f32 * 0.13;
            let (s, c) = angle.sin_cos();
            let nx = c * px - s * py;
            let ny = s * px + c * py;
            px = nx;
            py = ny;
        }
        if norm > 0.0 {
            sum / norm * d.amplitude_m
        } else {
            0.0
        }
    }

    fn ridges(&self, x: f32, y: f32, footprint: f32, variation: f32) -> f32 {
        let r = &self.settings.ridges;
        if !r.enabled || r.amplitude_m <= 0.0 {
            return 0.0;
        }
        let scale = r.scale_m * (0.65 + 0.75 * variation);
        let band = band_limit(scale, footprint);
        if band <= 0.0 {
            return 0.0;
        }
        // The direction wanders at the same low frequency as the patch field:
        // parallel crests read as corduroy, wandering ones read as rock.
        let jitter = (self
            .ridge_noise
            .noise2(x / (r.scale_m * 9.0), y / (r.scale_m * 9.0)))
            * 0.55;
        let angle = r.angle_deg.to_radians() + jitter + (variation - 0.5) * 0.7;
        let (s, c) = angle.sin_cos();
        let u = x * c + y * s;
        let v = -x * s + y * c;
        let anisotropy = r.anisotropy.max(0.05);
        let n = self.ridge_noise.ridged2(
            u / scale,
            v / (scale * anisotropy),
            6,
            self.settings.lacunarity,
            self.settings.gain,
        );
        // Sharpen the crest: `sharpness = 1` keeps the raw ridge, lower values
        // round it back toward a smooth slope.
        let crest = (n * 2.0 - 1.0).clamp(-1.0, 1.0);
        let crest = crest.signum() * crest.abs().powf(1.0 / (0.35 + r.sharpness));
        let amplitude = r.amplitude_m * (0.45 + 1.15 * variation);
        crest * amplitude * band
    }

    fn grooves(&self, x: f32, y: f32, footprint: f32, variation: f32) -> f32 {
        let g = &self.settings.grooves;
        if !g.enabled || g.depth_m <= 0.0 {
            return 0.0;
        }
        let spacing = g.spacing_m * (0.65 + 0.80 * variation);
        let band = band_limit(spacing * 2.0, footprint);
        if band <= 0.0 {
            return 0.0;
        }
        let jitter = (self
            .groove_noise
            .noise2(x / (g.spacing_m * 11.0), y / (g.spacing_m * 11.0)))
            * 0.45;
        let angle = g.angle_deg.to_radians() + jitter + (variation - 0.5) * 1.1;
        let (s, c) = angle.sin_cos();
        let u = x * c + y * s;
        let v = -x * s + y * c;
        let wander = self
            .groove_noise
            .fbm2(u / (spacing * 7.0), v / (spacing * 7.0), 3, 2.0, 0.5)
            * g.wander
            * spacing
            * 0.9;
        let phase = (u + wander) / spacing * std::f32::consts::TAU;
        // A cosine trough is smooth; the exponent narrows it into a flute.
        let channel = (0.5 + 0.5 * phase.cos()).powf(0.6 + g.sharpness * 3.0);
        let steep = self.slope_mask(x, y);
        let exposure = smoothstep(0.08, 0.50, steep);
        let depth = g.depth_m * (0.55 + 0.90 * variation);
        -depth * channel * band * exposure
    }

    fn craters(&self, x: f32, y: f32, footprint: f32, variation: f32) -> f32 {
        let c = &self.settings.craters;
        if !c.enabled || c.depth_m <= 0.0 || c.density <= 0.0 {
            return 0.0;
        }
        let band = band_limit(c.radius_m * 2.0, footprint);
        if band <= 0.0 {
            return 0.0;
        }
        let density = (c.density * (0.35 + 1.35 * variation)).clamp(0.0, 1.0);
        if density <= 0.0 {
            return 0.0;
        }
        // Jitter the grid itself before hashing: hash cells alone still line
        // up along the axes and read as a lattice at grazing angles.
        let cell = c.spacing_m.max(2.0);
        let warp_x = self.crater_noise.noise2(x / (cell * 3.0), y / (cell * 3.0)) * cell * 0.35;
        let warp_y = self
            .crater_noise
            .noise2(x / (cell * 3.0) + 9.1, y / (cell * 3.0) - 4.7)
            * cell
            * 0.35;
        let gx = (x + warp_x) / cell;
        let gy = (y + warp_y) / cell;
        let ix = gx.floor() as i32;
        let iy = gy.floor() as i32;
        let mut sum = 0.0f32;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let cx = ix + dx;
                let cy = iy + dy;
                let h = hash01(cx, cy, self.seed);
                if h > density {
                    continue;
                }
                let px = (cx as f32 + 0.12 + 0.76 * hash01(cx + 17, cy - 9, self.seed)) * cell;
                let py = (cy as f32 + 0.12 + 0.76 * hash01(cx - 5, cy + 23, self.seed)) * cell;
                let radius = c.radius_m * (0.35 + 1.3 * hash01(cx + 3, cy + 7, self.seed));
                let depth = c.depth_m * (0.4 + 0.8 * hash01(cx - 11, cy - 3, self.seed));
                let ddx = x - px;
                let ddy = y - py;
                let distance = (ddx * ddx + ddy * ddy).sqrt() / radius.max(0.2);
                if distance < 1.0 {
                    let bowl = (1.0 - distance * distance).max(0.0).powi(2);
                    sum -= depth * bowl * band;
                }
                let rim = depth * c.rim * band;
                if rim > 0.0 {
                    let ring = (-(distance - 1.0).powi(2) / 0.020).exp();
                    sum += rim * ring;
                }
                if c.ejecta > 0.0 && distance < 1.0 + c.ejecta {
                    let t = ((distance - 1.0) / c.ejecta.max(0.01)).clamp(0.0, 1.0);
                    let ejecta = (1.0 - t).powi(2);
                    let grain = self
                        .crater_noise
                        .noise2(x / (radius * 0.35), y / (radius * 0.35));
                    sum += depth * c.ejecta * 0.12 * ejecta * grain * band;
                }
            }
        }
        sum
    }

    fn exfoliation(&self, x: f32, y: f32, footprint: f32, variation: f32) -> f32 {
        let e = &self.settings.exfoliation;
        if !e.enabled || e.amplitude_m <= 0.0 {
            return 0.0;
        }
        // Plate size and layer count drift with the patch field. A constant
        // layer count produces contours at exactly the same spacing across the
        // whole mountain; that is the regularity this breaks.
        let scale = e.scale_m * (0.55 + 0.95 * variation);
        let band = band_limit(scale * 2.0, footprint);
        if band <= 0.0 {
            return 0.0;
        }
        let patch = smoothstep(
            0.28,
            0.72,
            self.exfoliation_noise.fbm01(
                x / (e.scale_m * 6.0) + 41.0,
                y / (e.scale_m * 6.0) - 7.0,
                2,
                2.0,
                0.5,
            ),
        );
        if patch <= 0.0 {
            return 0.0;
        }
        let steps = (e.layers.max(1) as f32 * (1.6 + 1.8 * variation)).clamp(1.0, 14.0);
        let f = self
            .exfoliation_noise
            .fbm2(x / scale, y / scale, 5, 2.0, 0.55);
        let q = (f * steps).round() / steps;
        // `q - f` is zero at a plate centre and largest at a plate edge;
        // scaling by `steps` makes the relief independent of the layer count.
        let shell = (q - f) * 2.0;
        let edge = 1.0 - smoothstep(0.0, 0.10, (f - q).abs());
        let amplitude = e.amplitude_m * (0.55 + 0.90 * variation);
        amplitude * shell * (0.55 + 0.45 * edge) * patch * band
    }

    fn vesicles(&self, x: f32, y: f32, footprint: f32, variation: f32) -> f32 {
        let v = &self.settings.vesicles;
        if !v.enabled || v.depth_m <= 0.0 || v.density <= 0.0 {
            return 0.0;
        }
        let spacing = v.spacing_m * (0.70 + 0.70 * variation);
        let band = band_limit(spacing * 2.0, footprint);
        if band <= 0.0 {
            return 0.0;
        }
        // `1 - |noise|` creates rounded cells; a threshold turns the cell
        // centres into pits and leaves the cell walls intact.
        let n = 1.0 - self.vesicle_noise.noise2(x / spacing, y / spacing).abs();
        let density = (v.density * (0.45 + 1.10 * variation)).clamp(0.0, 1.0);
        let threshold = 1.0 - density * 0.75;
        let pit = smoothstep(threshold, 1.0, n);
        -v.depth_m * pit * pit * band
    }

    /// Tafoni / honeycomb: warped cellular bowls on exposed faces.
    fn tafoni(&self, x: f32, y: f32, footprint: f32, slope: f32, variation: f32) -> f32 {
        let t = &self.settings.tafoni;
        if !t.enabled || t.depth_m <= 0.0 || t.cell_m <= 0.0 {
            return 0.0;
        }
        let band = band_limit(t.cell_m, footprint);
        if band <= 0.0 {
            return 0.0;
        }
        let cell = t.cell_m.max(0.05);
        let warp_scale = (cell * 4.0).max(1.0);
        let wx = self
            .tafoni_noise
            .fbm2(x / warp_scale, y / warp_scale, 3, 2.0, 0.5)
            * cell
            * 0.35;
        let wy = self
            .tafoni_noise
            .fbm2(x / warp_scale + 13.0, y / warp_scale - 7.0, 3, 2.0, 0.5)
            * cell
            * 0.35;
        let (f1, _, id) = cellular(x + wx, y + wy, cell, self.seed ^ 0x0007_A0F1);
        let coverage = smoothstep(1.0 - t.coverage, 1.0, id);
        let bowl = (1.0 - smoothstep(0.0, 0.72, f1))
            .max(0.0)
            .powf(1.0 + t.sharpness);
        let rim = smoothstep(0.45, 0.62, f1) * (1.0 - smoothstep(0.62, 0.82, f1));
        let patch = if t.patchiness > 0.0 {
            let p = self
                .tafoni_noise
                .fbm01(x / (cell * 12.0), y / (cell * 12.0), 3, 2.0, 0.5);
            (1.0 - t.patchiness) + t.patchiness * smoothstep(0.35, 0.7, p)
        } else {
            1.0
        };
        let vertical = 1.0 - t.vertical_bias + t.vertical_bias * smoothstep(0.22, 0.62, slope);
        let scale = (0.8 + 0.4 * variation).clamp(0.5, 1.4);
        (-t.depth_m * bowl + t.depth_m * 0.22 * rim) * coverage * patch * vertical * scale * band
    }

    /// Sedimentary beds: hard ledges and softer recesses, dipping.
    fn benches(&self, x: f32, y: f32, footprint: f32, slope: f32, variation: f32) -> f32 {
        let b = &self.settings.benches;
        if !b.enabled || b.thickness_m <= 0.0 || b.relief_m <= 0.0 {
            return 0.0;
        }
        let band = band_limit(b.thickness_m, footprint);
        if band <= 0.0 {
            return 0.0;
        }
        let dip = b.dip_deg.to_radians().tan();
        let az = b.dip_azimuth_deg.to_radians();
        let along = x * az.cos() + y * az.sin();
        let base = self.base_height(f64::from(x), f64::from(y));
        let s = base - dip * along;
        let warp = self.bench_noise.fbm2(
            x / (b.thickness_m * 5.0),
            y / (b.thickness_m * 5.0),
            3,
            2.0,
            0.5,
        ) * b.variation;
        let phase = s / b.thickness_m + warp;
        let f = phase - phase.floor();
        let riser = smoothstep(0.0, (1.0 - b.sharpness * 0.85).max(0.02), f);
        let profile = riser - 0.5;
        let slope_mask = smoothstep(0.12, 0.5, slope);
        let scale = (0.75 + 0.5 * variation).clamp(0.4, 1.5);
        b.relief_m * profile * slope_mask * scale * band
    }

    /// Two orthogonal joint sets, carved as narrow grooves.
    fn joints(&self, x: f32, y: f32, footprint: f32, _variation: f32) -> f32 {
        let j = &self.settings.joints;
        if !j.enabled || j.spacing_m <= 0.0 || j.depth_m <= 0.0 {
            return 0.0;
        }
        let band = band_limit(j.spacing_m, footprint);
        if band <= 0.0 {
            return 0.0;
        }
        let seed = self.seed ^ 0x0004_A017;
        let d1 = grid_distance(x, y, j.spacing_m, j.angle_deg.to_radians(), j.jitter, seed);
        let d2 = grid_distance(
            x,
            y,
            j.spacing_m,
            j.angle2_deg.to_radians(),
            j.jitter,
            seed + 31,
        );
        let width = j.width_m.max(0.01);
        let seam = 1.0 - smoothstep(width * 0.35, width * 0.35 + width, d1.min(d2));
        -j.depth_m * seam * band
    }

    /// Oolitic / bioclastic grains: rounded growths proud of the matrix.
    fn ooliths(&self, x: f32, y: f32, footprint: f32, slope: f32, _variation: f32) -> f32 {
        let o = &self.settings.ooliths;
        if !o.enabled || o.radius_m <= 0.0 || o.relief_m <= 0.0 {
            return 0.0;
        }
        let cell = (o.radius_m * 2.4).max(0.01);
        let band = band_limit(cell, footprint);
        if band <= 0.0 {
            return 0.0;
        }
        let (f1, _, id) = cellular(x, y, cell, self.seed ^ 0x0000_0117);
        let coverage = smoothstep(1.0 - o.coverage, 1.0, id);
        let bump = (1.0 - smoothstep(0.0, 0.62, f1)).max(0.0).powf(0.65);
        let size = 1.0 + (value2(x / cell, y / cell, self.seed) - 0.5) * o.variation;
        let slope_mask = 0.35 + 0.65 * (1.0 - smoothstep(0.45, 0.8, slope));
        o.relief_m * bump * coverage * size * slope_mask * band
    }
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if (edge1 - edge0).abs() < f32::EPSILON {
        return if x >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Distance to the nearest line of a rotated 2D grid, in metres.
fn grid_distance(x: f32, y: f32, spacing: f32, angle_rad: f32, jitter: f32, seed: u64) -> f32 {
    let (s, c) = angle_rad.sin_cos();
    let u = x * c + y * s;
    let v = -x * s + y * c;
    let ju = (value2(u / spacing, 0.0, seed) - 0.5) * jitter * spacing;
    let jv = (value2(0.0, v / spacing, seed + 1) - 0.5) * jitter * spacing;
    let fu = (u / spacing + ju / spacing + 0.5).fract() - 0.5;
    let fv = (v / spacing + jv / spacing + 0.5).fract() - 0.5;
    (fu.abs().min(fv.abs())) * spacing
}

/// Warped cellular field: (F1, F2, cell id) in cell units.
fn cellular(x: f32, y: f32, cell: f32, seed: u64) -> (f32, f32, f32) {
    let px = x / cell;
    let py = y / cell;
    let cx = px.floor();
    let cy = py.floor();
    let mut f1 = f32::INFINITY;
    let mut f2 = f32::INFINITY;
    let mut id = 0.0;
    for oy in -1..=1 {
        for ox in -1..=1 {
            let gx = cx + ox as f32;
            let gy = cy + oy as f32;
            let jx = value2(gx, gy, seed);
            let jy = value2(gx + 17.0, gy - 9.0, seed + 3);
            let rx = gx + jx;
            let ry = gy + jy;
            let d = (rx - px) * (rx - px) + (ry - py) * (ry - py);
            if d < f1 {
                f2 = f1;
                f1 = d;
                id = value2(gx, gy, seed + 11);
            } else if d < f2 {
                f2 = d;
            }
        }
    }
    (f1.sqrt(), f2.sqrt(), id)
}

/// Fade a wavelength out when it is close to the pixel footprint. Below
/// `~1.5` samples per wavelength the layer is invisible; above `~3` it is
/// fully resolved.
#[inline]
fn band_limit(wavelength: f32, footprint: f32) -> f32 {
    if footprint <= 0.02 || wavelength <= 0.0 {
        return 1.0;
    }
    smoothstep(1.5, 3.0, wavelength / footprint)
}

#[inline]
fn hash01(x: i32, y: i32, seed: u64) -> f32 {
    (hash_u64(x, y, seed) >> 40) as f32 * (1.0 / 16_777_216.0)
}

#[inline]
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
    use vf_core::{ClassicParams, DetailSettings};

    fn field() -> HeightField {
        vf_procgen::classic::generate(&ClassicParams::default().with_size_seed(129, 7)).unwrap()
    }

    #[test]
    fn detail_is_deterministic_and_changes_height() {
        let hf = field();
        let settings = DetailSettings::preset("granite").unwrap();
        let a = DetailSampler::new(&hf, &settings, Seed::new(7));
        let b = DetailSampler::new(&hf, &settings, Seed::new(7));
        let x = 1234.5;
        let y = 678.9;
        assert_eq!(a.height(x, y, 1.0).to_bits(), b.height(x, y, 1.0).to_bits());
        let base = a.base_height(x, y);
        let detailed = a.height(x, y, 1.0);
        assert!((detailed - base).abs() > 1e-4, "detail is a no-op");
    }

    #[test]
    fn new_geometry_layers_produce_relief_and_are_deterministic() {
        let hf = field();
        let x = 820.0;
        let y = 940.0;
        for layer in 0..4 {
            let mut settings = DetailSettings {
                enabled: true,
                strength: 1.0,
                ..DetailSettings::default()
            };
            // Isolate one layer at a time: the fractal stays, but the
            // layer-specific fields are disabled except the tested one.
            settings.tafoni.enabled = false;
            settings.benches.enabled = false;
            settings.joints.enabled = false;
            settings.ooliths.enabled = false;
            match layer {
                0 => {
                    settings.tafoni.enabled = true;
                    settings.tafoni.cell_m = 1.4;
                    settings.tafoni.depth_m = 0.5;
                    settings.tafoni.coverage = 1.0;
                    settings.tafoni.patchiness = 0.0;
                }
                1 => {
                    settings.benches.enabled = true;
                    settings.benches.thickness_m = 2.0;
                    settings.benches.relief_m = 1.0;
                    settings.benches.sharpness = 0.8;
                }
                2 => {
                    settings.joints.enabled = true;
                    settings.joints.spacing_m = 4.0;
                    settings.joints.depth_m = 0.4;
                    settings.joints.width_m = 0.4;
                }
                _ => {
                    settings.ooliths.enabled = true;
                    settings.ooliths.radius_m = 0.35;
                    settings.ooliths.coverage = 1.0;
                    settings.ooliths.relief_m = 0.3;
                }
            }
            let a = DetailSampler::new(&hf, &settings, Seed::new(11));
            let b = DetailSampler::new(&hf, &settings, Seed::new(11));
            let da = a.detail(x, y, 0.1);
            let db = b.detail(x, y, 0.1);
            assert_eq!(
                da.to_bits(),
                db.to_bits(),
                "layer {layer} is not deterministic"
            );
            let mut different = false;
            for dx in 0..12 {
                let d = a.detail(x + dx as f64 * 0.7, y + dx as f64 * 0.4, 0.1);
                if (d - da).abs() > 1e-4 {
                    different = true;
                    break;
                }
            }
            assert!(different, "layer {layer} has no spatial variation");
        }
    }

    #[test]
    fn disabled_detail_is_identity() {
        let hf = field();
        let settings = DetailSettings::default();
        let sampler = DetailSampler::new(&hf, &settings, Seed::new(1));
        for (x, y) in [(10.0, 10.0), (500.0, 300.0), (1500.0, 900.0)] {
            assert!((sampler.height(x, y, 0.0) - sampler.base_height(x, y)).abs() < 1e-6);
        }
    }

    #[test]
    fn band_limit_attenuates_high_frequencies() {
        assert_eq!(band_limit(20.0, 0.0), 1.0);
        assert!(band_limit(1.0, 10.0) < 0.01);
        assert!(band_limit(50.0, 10.0) > 0.99);
    }

    #[test]
    fn normals_are_unit_and_upward_on_flat_ground() {
        let hf = HeightField::filled(33, 33, 10.0, 100.0).unwrap();
        let mut settings = DetailSettings::preset("granite").unwrap();
        settings.strength = 0.0;
        let sampler = DetailSampler::new(&hf, &settings, Seed::new(2));
        let n = sampler.normal(100.0, 100.0, 1.0);
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        assert!((len - 1.0).abs() < 1e-4);
        assert!(n[2] > 0.99);
    }

    #[test]
    fn profiles_produce_different_surfaces() {
        let hf = field();
        let granite = DetailSettings::preset("granite").unwrap();
        let chalk = DetailSettings::preset("chalk").unwrap();
        let a = DetailSampler::new(&hf, &granite, Seed::new(3));
        let b = DetailSampler::new(&hf, &chalk, Seed::new(3));
        let mut different = 0;
        for i in 0..20 {
            let x = 200.0 + i as f64 * 37.0;
            let y = 300.0 + i as f64 * 13.0;
            if (a.detail(x, y, 0.5) - b.detail(x, y, 0.5)).abs() > 1e-3 {
                different += 1;
            }
        }
        assert!(different > 10, "profiles are too similar");
    }
}
