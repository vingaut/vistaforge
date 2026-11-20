//! Surface, water and sky shading for the CPU renderer.

use glam::Vec3;
use vf_core::{HeightField, Palette, Project, Rgb, TerrainStats};

use crate::RenderOptions;
use crate::SceneData;
use crate::shadow::ShadowMap;
use crate::smoothstep;
use vf_material::{RockStyle, RockTexture};

pub struct ShadeCtx<'a> {
    project: &'a Project,
    hf: &'a HeightField,
    scene: &'a SceneData,
    options: &'a RenderOptions,
    shadow: Option<&'a ShadowMap>,
    sea_level: f32,
    cloud_altitude: f32,
    stats: TerrainStats,
    palette: Palette,
    /// Custom rock texture loaded once per render.
    texture: Option<RockTexture>,
}

impl<'a> ShadeCtx<'a> {
    pub(crate) fn new(
        project: &'a Project,
        hf: &'a HeightField,
        scene: &'a SceneData,
        options: &'a RenderOptions,
        shadow: Option<&'a ShadowMap>,
    ) -> Self {
        let camera_altitude = scene.center[2] as f32 + scene.camera_pos.z;
        Self {
            project,
            hf,
            scene,
            options,
            shadow,
            sea_level: project.water.sea_level,
            // Keep the cloud layer above the eye so sky rays always hit it.
            cloud_altitude: project.clouds.altitude_m.max(camera_altitude + 900.0),
            stats: hf.stats(),
            palette: project.palette.clone(),
            texture: RockTexture::load(&project.materials.texture_path),
        }
    }

    pub fn sky(&self, dir: Vec3) -> [f32; 3] {
        let up = dir.z.clamp(-1.0, 1.0);
        // A narrow warm band at the horizon and a dark zenith: this is what
        // makes a dusk sky read, and it also gives daytime a crisp horizon.
        // Sharp horizon glow: the warm band must stay within ~10 degrees,
        // otherwise the whole sky turns amber and the zenith never reads.
        let band = (1.0 - smoothstep(0.0, 0.22, up.max(0.0))).powf(2.2);
        let mut color = self.palette.sky_zenith.lerp(self.palette.sky_horizon, band);
        if up < 0.0 {
            color = self
                .palette
                .fog
                .lerp(self.palette.sky_horizon, (1.0 + up).max(0.0));
        }
        color = color.scale(self.project.atmosphere.sky_intensity.max(0.0));

        let cos_sun = dir.dot(self.scene.sun_dir).clamp(-1.0, 1.0);
        let glow = cos_sun.max(0.0).powf(64.0) * 0.55 + cos_sun.max(0.0).powf(8.0) * 0.18;
        let sun = self
            .scene
            .sun_color
            .scale(glow * self.project.sun.intensity);

        let radius = self.project.sun.angular_radius_deg.to_radians();
        let disk = if cos_sun.acos() <= radius {
            self.scene
                .sun_color
                .scale(12.0 * self.project.sun.intensity)
        } else {
            Rgb::BLACK
        };

        if self.project.atmosphere.stars > 0.0 && up > 0.0 {
            let field = star_field(dir, self.project.seed.0);
            // Stars only where the sky is dark: never in the horizon glow.
            let dark = (1.0 - band).powf(3.0) * smoothstep(0.18, 0.45, up);
            color = color
                + Rgb::new(0.92, 0.95, 1.0)
                    .scale(field * self.project.atmosphere.stars * 2.2 * dark);
        }

        let clouds = if self.options.clouds && self.project.clouds.enabled && dir.z > 0.01 {
            self.cloud_layer(dir)
        } else {
            Rgb::BLACK
        };

        color = color + (sun) + (disk);
        let cloud_alpha = if self.options.clouds && self.project.clouds.enabled {
            self.cloud_alpha(dir)
        } else {
            0.0
        };
        color = color.lerp(clouds, cloud_alpha);
        color.to_array()
    }

    fn cloud_layer(&self, dir: Vec3) -> Rgb {
        let clouds = &self.project.clouds;
        let alt_rel = self.cloud_altitude - self.scene.center[2] as f32;
        let t = (alt_rel - self.scene.camera_pos.z) / dir.z;
        if t <= 0.0 {
            return self.palette.cloud_lit;
        }
        let p = self.scene.camera_pos + dir * t;
        let x = p.x + self.scene.center[0] as f32;
        let y = p.y + self.scene.center[1] as f32;
        let density = self.cloud_density(x, y) * clouds.density.clamp(0.0, 1.0);
        let lit = 0.55 + 0.45 * self.scene.sun_dir.z.max(0.0);
        self.palette
            .cloud_shadow
            .lerp(self.palette.cloud_lit, lit)
            .scale(0.85 + 0.15 * density)
    }

    fn cloud_alpha(&self, dir: Vec3) -> f32 {
        let clouds = &self.project.clouds;
        let alt_rel = self.cloud_altitude - self.scene.center[2] as f32;
        let t = (alt_rel - self.scene.camera_pos.z) / dir.z;
        if t <= 0.0 {
            return 0.0;
        }
        let p = self.scene.camera_pos + dir * t;
        let x = p.x + self.scene.center[0] as f32;
        let y = p.y + self.scene.center[1] as f32;
        let density = self.cloud_density(x, y) * clouds.density.clamp(0.0, 1.0);
        // Grazing rays travel tens of kilometres through the layer and turn
        // into a striped band on the horizon: fade them out well before that.
        let angle = smoothstep(0.02, 0.12, dir.z);
        let distance = 1.0 / (1.0 + t / 60_000.0);
        (density * 0.96).clamp(0.0, 0.97) * angle * distance
    }

    fn cloud_density(&self, x_abs: f32, y_abs: f32) -> f32 {
        crate::cloud_density_at(self.project, x_abs, y_abs)
    }

    pub fn terrain(&self, pos_rel: Vec3, normal: Vec3, ao: f32) -> [f32; 3] {
        let pos_abs = self.world(pos_rel);
        let albedo = self.albedo(pos_abs, normal);
        self.surface(pos_rel, normal, ao, albedo)
    }

    /// Shared direct lighting, shadow, sky fill and haze for a surface with an
    /// already-computed albedo.
    fn surface(&self, pos_rel: Vec3, normal: Vec3, ao: f32, albedo: Rgb) -> [f32; 3] {
        let pos_abs = self.world(pos_rel);
        let distance = (pos_rel - self.scene.camera_pos).length();
        // Light the perturbed normal, but keep the geometric normal for the
        // material masks: micro-relief must not move the snow line.
        let shade_n = self.detail_normal(normal, pos_abs, distance);
        let ndl = shade_n.dot(self.scene.sun_dir).max(0.0);
        let visibility = if self.options.shadows {
            self.shadow_at(pos_abs, normal)
        } else {
            1.0
        };
        let cloud_shadow = if self.options.clouds {
            self.cloud_shadow(pos_abs)
        } else {
            1.0
        };
        let sun = self
            .scene
            .sun_color
            .scale(ndl * visibility * cloud_shadow * self.project.sun.intensity);
        // Sky fill: a shadowed slope is lit by the whole sky, not only by the
        // zenith, and ambient occlusion must dim it, not extinguish it. A
        // vertical face sees the bright horizon band, so it gets more ambient
        // than the old `nz` term allowed; otherwise a canyon wall in shadow
        // turns black.
        // HBAO-style contrast curve: values around 0.5 are pushed apart so
        // that a faint 0.75 occlusion still reads as a crease.
        let ao = hbao_gain(ao.clamp(0.0, 1.0), 3.0);
        let sky_mix = 0.45 + 0.35 * (1.0 - shade_n.z.max(0.0));
        let sky_ambient = self
            .palette
            .sky_zenith
            .lerp(self.palette.sky_horizon, sky_mix)
            .scale(
                self.project.atmosphere.ambient_strength
                    * self.project.atmosphere.sky_intensity
                    * (0.60 + 0.40 * shade_n.z.max(0.0))
                    * (0.55 + 0.45 * ao),
            );
        // Warm ground bounce on vertical faces: the sunlit floor of a canyon
        // is a large, bright surface. This is the cheap local version of a
        // global-illumination bounce.
        let bounce = self.palette.sand.lerp(self.palette.cliff, 0.35).scale(
            self.project.atmosphere.ambient_strength
                * 0.28
                * (1.0 - shade_n.z.max(0.0))
                * (0.40 + 0.60 * ao),
        );
        // Apply the occlusion to the whole shaded result, not only to the
        // ambient term: that is what makes a cavity read as a cavity when the
        // sun is directly on it. `pow(ao, 0.5)` matches the HBAO reference.
        let color = (albedo * (sun + (sky_ambient) + (bounce))).scale(ao.max(0.05).powf(0.5));
        self.haze(color, pos_rel).to_array()
    }

    /// Perturb the geometric normal with a fractal surface so close surfaces
    /// catch the light like rock instead of smooth clay.
    fn detail_normal(&self, n: Vec3, p: Vec3, distance: f32) -> Vec3 {
        let mats = &self.project.materials;
        if !mats.enabled
            || (mats.bump_strength <= 0.0 && mats.texture_bump <= 0.0)
            || mats.bump_scale_m <= 0.0
        {
            return n;
        }
        // Fade out with distance: high-frequency detail would alias the ridges.
        let mut fade = 1.0 / (1.0 + distance / 2500.0);
        // Steep faces already carry their relief in the geometry (the cliff
        // staircase). Perturbing their normals again turns the one-cell
        // risers into a dense corduroy, so the detail is both weaker and
        // broader on cliffs.
        let slope = (1.0 - n.z).clamp(0.0, 1.0);
        let steep = smoothstep(0.30, 0.65, slope);
        fade *= 1.0 - 0.95 * steep;
        let scale = mats.bump_scale_m.max(0.05) * (1.0 + 1.5 * steep);
        let eps = scale * 0.75;
        let noise = &self.scene.noise;
        // Tangent frame on the surface: the detail follows the face instead of
        // sliding in world XY, which is what made cliffs look smeared.
        let reference = if n.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
        let tangent = n.cross(reference).normalize_or_zero();
        let bitangent = n.cross(tangent).normalize_or_zero();
        let f = |u: f32, v: f32| noise.fbm01(u / scale, v / scale, 3, 2.0, 0.5);
        let u = p.dot(tangent);
        let v = p.dot(bitangent);
        let h0 = f(u, v);
        let hu = f(u + eps, v);
        let hv = f(u, v + eps);
        let k = mats.bump_strength * 2.0 * fade;
        let mut perturbed = n - tangent * ((hu - h0) * k) - bitangent * ((hv - h0) * k);
        // Custom texture bump: luminance gradient in tile space. The texture
        // is sampled in world XY so it stays stable while the camera moves.
        if let Some(texture) = &self.texture
            && mats.texture_bump > 0.0
        {
            let tile = mats.texture_scale_m.max(0.1);
            let u = p.x / tile;
            let v = p.y / tile;
            let e = 1.0 / 512.0;
            let h0 = texture.luminance(u, v);
            let hu = texture.luminance(u + e, v);
            let hv = texture.luminance(u, v + e);
            let t = mats.texture_bump * 2.2 * fade;
            perturbed -= tangent * ((hu - h0) * t) + bitangent * ((hv - h0) * t);
        }
        perturbed.normalize_or_zero()
    }

    pub fn ground(&self, pos_rel: Vec3, normal: Vec3) -> [f32; 3] {
        let pos_abs = self.world(pos_rel);
        let ndl = normal.dot(self.scene.sun_dir).max(0.0);
        let variation =
            self.scene
                .noise
                .fbm01(pos_abs.x * 0.00035, pos_abs.y * 0.00035, 4, 2.0, 0.5);
        let base = if matches!(
            self.palette.name.as_str(),
            "desert" | "canyon" | "lunar" | "volcanic" | "dusk"
        ) {
            self.palette.sand.scale(0.82)
        } else {
            self.palette
                .grass
                .lerp(self.palette.rock, 0.55 + 0.2 * variation)
                .scale(0.72)
        };
        let sun = self
            .scene
            .sun_color
            .scale(ndl * 0.9 * self.project.sun.intensity);
        let ambient = self
            .palette
            .sky_zenith
            .lerp(self.palette.sky_horizon, 0.4)
            .scale(self.project.atmosphere.ambient_strength * 0.55);
        let color = base * (sun + (ambient));
        self.haze(color, pos_rel).to_array()
    }

    pub fn skirt(&self, pos_rel: Vec3, normal: Vec3) -> [f32; 3] {
        let ndl = normal.dot(self.scene.sun_dir).max(0.0);
        let base = self.palette.cliff.scale(0.62);
        let sun = self
            .scene
            .sun_color
            .scale(ndl * 0.85 * self.project.sun.intensity);
        let ambient = self
            .palette
            .fog
            .scale(self.project.atmosphere.ambient_strength * 0.45);
        let color = base * (sun + (ambient));
        self.haze(color, pos_rel).to_array()
    }

    /// Returns `None` when the water plane is above the terrain at this pixel.
    pub fn water(&self, pos_rel: Vec3) -> Option<[f32; 3]> {
        let pos_abs = self.world(pos_rel);
        let (terrain_h, inside, outside_distance) = self.sample_abs_inside(pos_abs.x, pos_abs.y);
        let shelf = (self.sea_level - terrain_h).max(0.0);
        let depth = if inside {
            shelf
        } else {
            shelf.max(0.5) + outside_distance * 0.01
        };
        if depth <= 0.0 {
            return None;
        }
        let n = self.wave_normal(pos_abs.x, pos_abs.y);
        let view = (self.scene.camera_pos - pos_rel).normalize_or_zero();
        let fresnel = 0.02 + 0.98 * (1.0 - n.dot(view).clamp(0.0, 1.0)).powi(5);
        let reflected = self.sky((-view).reflect(n).normalize_or_zero());
        let deep = self
            .palette
            .water_shallow
            .lerp(self.palette.water_deep, smoothstep(0.0, 90.0, depth));
        let sun_spec = {
            let r = (-self.scene.sun_dir).reflect(n);
            r.dot(view).max(0.0).powf(240.0)
        };
        let shadow = if self.options.shadows {
            self.shadow_at(pos_abs, Vec3::Z)
        } else {
            1.0
        };
        let sun = self
            .scene
            .sun_color
            .scale(sun_spec * 1.3 * shadow * self.project.sun.intensity);
        let body = deep.scale(0.72);
        let mut color =
            body.lerp(Rgb::new(reflected[0], reflected[1], reflected[2]), fresnel) + (sun);

        let shore = (1.0 - smoothstep(0.0, 2.5, depth)) * self.project.water.foam;
        if shore > 0.0 {
            let foam_noise =
                self.scene
                    .noise
                    .fbm01(pos_abs.x * 0.03, pos_abs.y * 0.03, 4, 2.0, 0.5);
            color = color.lerp(self.palette.foam, shore * (0.35 + 0.65 * foam_noise));
        }
        Some(self.haze(color, pos_rel).to_array())
    }

    fn wave_normal(&self, x: f32, y: f32) -> Vec3 {
        let water = &self.project.water;
        let freq = 0.004 + water.wave_scale.max(0.0) * 0.12;
        let eps = 4.0f32;
        let n = self.scene.noise;
        let h0 = n.fbm2(x * freq, y * freq, 4, 2.0, 0.5);
        let hx = n.fbm2((x + eps) * freq, y * freq, 4, 2.0, 0.5);
        let hy = n.fbm2(x * freq, (y + eps) * freq, 4, 2.0, 0.5);
        let strength = water.wave_strength.max(0.0) * 0.35;
        Vec3::new(
            -(hx - h0) / eps * strength,
            -(hy - h0) / eps * strength,
            1.0,
        )
        .normalize_or_zero()
    }

    fn albedo(&self, pos_abs: Vec3, normal: Vec3) -> Rgb {
        let style = &self.project.style;
        let mats = &self.project.materials;
        let relief = (self.stats.max - self.stats.min).max(1.0);
        let alt = pos_abs.z;
        let rel = ((alt - self.stats.min) / relief).clamp(0.0, 1.0);
        let slope = (1.0 - normal.z).clamp(0.0, 1.0);
        let noise = &self.scene.noise;

        let macro_n = noise.fbm01(pos_abs.x * 0.0007, pos_abs.y * 0.0007, 4, 2.0, 0.5);
        let fine = noise.fbm01(pos_abs.x * 0.0045, pos_abs.y * 0.0045, 3, 2.0, 0.5);
        let detail = noise.fbm01(pos_abs.x * 0.02, pos_abs.y * 0.02, 2, 2.0, 0.5);

        // --- Vegetation: altitude, slope and patchiness ---------------------
        // Arid palettes get almost no vegetation: green patches in a canyon
        // read as a bug.
        let arid = matches!(
            self.palette.name.as_str(),
            "desert" | "canyon" | "lunar" | "volcanic" | "dusk"
        );
        let veg = if !mats.enabled {
            0.55
        } else if arid {
            mats.vegetation * 0.12
        } else {
            mats.vegetation
        };
        let tree = 1.0 - smoothstep(style.tree_line_m - 180.0, style.tree_line_m + 220.0, alt);
        let gentle = 1.0 - smoothstep(0.16, 0.40, slope);
        let patch = smoothstep(0.25, 0.75, fine);
        let veg_mask = (tree * gentle * (0.85 + 0.15 * patch) * veg).clamp(0.0, 1.0);

        let mut color = self.palette.grass.lerp(
            self.palette.forest,
            smoothstep(0.08, 0.45, rel) * (0.6 + 0.4 * macro_n),
        );
        // Dry, sun-bleached patches on the low ground.
        color = color.lerp(self.palette.sand, (1.0 - patch) * 0.10 * (1.0 - rel));

        // --- Rock: strata, joints and scree ---------------------------------
        let mut rock = self.palette.rock.scale(0.9 + 0.22 * macro_n);
        // Triplanar grain: a metre-scale value break-up so large faces stop
        // reading as smooth clay. Cheap, and it follows the surface via z.
        let grain = noise.fbm01(
            pos_abs.x * 0.09 + pos_abs.z * 0.04,
            pos_abs.y * 0.09 - pos_abs.z * 0.03,
            3,
            2.0,
            0.5,
        );
        let patches = noise.fbm01(pos_abs.x * 0.012, pos_abs.y * 0.012, 2, 2.0, 0.5);
        rock = rock.scale(0.78 + 0.34 * grain + 0.12 * patches);
        if mats.enabled {
            let style = RockStyle::preset(&mats.rock_preset);
            let seed_offset = (self.project.seed.0 % 4096) as f32 * 0.37;
            rock = style.shade(rock, pos_abs, slope, noise, seed_offset);
            if let Some(texture) = &self.texture {
                let tile = mats.texture_scale_m.max(0.1);
                let sampled = texture.sample(pos_abs.x / tile, pos_abs.y / tile);
                let sampled = Rgb::new(sampled[0], sampled[1], sampled[2]);
                // Keep the texture on rock faces; snow, grass and beach are
                // applied afterwards and keep their palette.
                rock = rock.lerp(sampled, mats.texture_mix.clamp(0.0, 1.0));
            }
        }
        let geology = &self.project.geology;
        if geology.enabled && geology.strata.enabled && geology.strata.color_contrast > 0.0 {
            let sample = vf_geology::strata_sample(
                &geology.strata,
                f64::from(pos_abs.x),
                f64::from(pos_abs.y),
                pos_abs.z,
                self.project.seed.derive("strata").0,
            );
            let band = self
                .palette
                .rock
                .lerp(self.palette.sand, 0.22 * sample.band)
                .lerp(self.palette.cliff, 0.55 * (1.0 - sample.band));
            let lit = 0.74 + 0.45 * sample.hardness;
            // Bands only read on faces, never across a meadow.
            let band_mask = smoothstep(0.28, 0.48, slope);
            rock = rock.lerp(band.scale(lit), geology.strata.color_contrast * band_mask);
        }
        // Fracture shadow: dark seams at the joint network.
        if geology.enabled && geology.joints.enabled {
            let cell = self.hf.spacing().0.max(self.hf.spacing().1);
            let joint = vf_geology::joint_mask(
                &geology.joints,
                f64::from(pos_abs.x),
                f64::from(pos_abs.y),
                cell,
                self.project.seed.derive("joints").0,
            );
            // Joint shading belongs on cliff faces; on gentle slopes the network
            // reads as a wireframe drawn on the ground.
            let seam = smoothstep(0.35, 0.62, slope);
            rock = rock.scale(1.0 - 0.18 * joint * seam);
        }
        // Scree: light debris on the slopes below the cliffs.
        if mats.scree > 0.0 {
            let band = smoothstep(mats.scree_slope - 0.08, mats.scree_slope + 0.12, slope)
                * (1.0 - smoothstep(0.62, 0.82, slope));
            let amount = mats.scree * band * (0.25 + 0.75 * fine) * 0.8;
            rock = rock.lerp(rock.scale(1.30), amount.clamp(0.0, 1.0));
        }
        color = color.lerp(rock, (1.0 - veg_mask).clamp(0.0, 1.0));

        // --- Snow: patches, not an altitude band ----------------------------
        let snow_alt = smoothstep(style.snow_line_m - 220.0, style.snow_line_m + 80.0, alt);
        let snow_slope = 1.0 - smoothstep(mats.snow_slope, mats.snow_slope + 0.16, slope);
        let coverage = mats.snow_coverage.clamp(0.0, 1.0);
        let snow_patch = smoothstep(
            0.55 - 0.5 * coverage,
            0.75 - 0.35 * coverage,
            macro_n * 0.6 + fine * 0.4,
        );
        let aspect = 1.0 - 0.30 * normal.dot(self.scene.sun_dir).max(0.0);
        let snow = (snow_alt * snow_slope * snow_patch * aspect).clamp(0.0, 1.0) * 0.97;
        color = color.lerp(self.palette.snow, snow);

        // --- Beach and wetness ----------------------------------------------
        let beach = 1.0 - smoothstep(style.beach_height_m, style.beach_height_m + 25.0, alt);
        color = color.lerp(self.palette.sand, beach * 0.9);
        let wet = mats.wetness * (1.0 - smoothstep(0.0, 12.0, (alt - self.sea_level).max(0.0)));
        if wet > 0.0 {
            color = color.lerp(self.palette.cliff, wet * 0.35);
        }
        if alt < self.sea_level {
            color = color.lerp(self.palette.water_deep, 0.45);
        }
        // Fine value variation keeps large faces from reading as flat paint.
        let variation =
            0.88 + 0.24 * (detail * 0.5 + macro_n * 0.5) * (0.5 + style.macro_variation);
        color.scale(variation.clamp(0.6, 1.4))
    }

    fn shadow_at(&self, pos_abs: Vec3, normal: Vec3) -> f32 {
        match self.shadow {
            Some(map) => map.sample(
                pos_abs,
                normal,
                self.scene.sun_dir,
                1.0 + self.project.sun.angular_radius_deg * 2.0,
            ),
            None => 1.0,
        }
    }

    fn cloud_shadow(&self, pos_abs: Vec3) -> f32 {
        let clouds = &self.project.clouds;
        if !clouds.enabled || self.scene.sun_dir.z <= 0.05 {
            return 1.0;
        }
        let t = (self.cloud_altitude - pos_abs.z) / self.scene.sun_dir.z;
        if t <= 0.0 {
            return 1.0;
        }
        let p = pos_abs + self.scene.sun_dir * t;
        let density = self.cloud_density(p.x, p.y);
        (1.0 - clouds.shadow_strength.clamp(0.0, 1.0) * density).clamp(0.0, 1.0)
    }

    fn haze(&self, color: Rgb, pos_rel: Vec3) -> Rgb {
        let atmosphere = &self.project.atmosphere;
        if !atmosphere.enabled || atmosphere.haze_density <= 0.0 {
            return color;
        }
        let distance = (pos_rel - self.scene.camera_pos).length();
        let pos_abs = self.world(pos_rel);
        let height = (pos_abs.z - self.sea_level).max(0.0);
        let density = atmosphere.haze_density * (-height * atmosphere.haze_falloff).exp();
        let amount = (1.0 - (-distance * density).exp()).clamp(0.0, 0.92);
        color.lerp(self.palette.fog, amount)
    }

    #[inline]
    fn world(&self, pos_rel: Vec3) -> Vec3 {
        pos_rel
            + Vec3::new(
                self.scene.center[0] as f32,
                self.scene.center[1] as f32,
                self.scene.center[2] as f32,
            )
    }

    fn sample_abs_inside(&self, x_abs: f32, y_abs: f32) -> (f32, bool, f32) {
        let (ox, oy) = self.hf.origin();
        let (sx, sy) = self.hf.spacing();
        let gx = (f64::from(x_abs) - ox) / sx;
        let gy = (f64::from(y_abs) - oy) / sy;
        let inside = gx >= 0.0
            && gy >= 0.0
            && gx <= (self.hf.width() - 1) as f64
            && gy <= (self.hf.height() - 1) as f64;
        let x0 = ox;
        let y0 = oy;
        let x1 = ox + (self.hf.width() - 1) as f64 * sx;
        let y1 = oy + (self.hf.height() - 1) as f64 * sy;
        let dx = if x_abs < x0 as f32 {
            f64::from(x0 as f32 - x_abs)
        } else if f64::from(x_abs) > x1 {
            f64::from(x_abs) - x1
        } else {
            0.0
        };
        let dy = if y_abs < y0 as f32 {
            f64::from(y0 as f32 - y_abs)
        } else if f64::from(y_abs) > y1 {
            f64::from(y_abs) - y1
        } else {
            0.0
        };
        let outside = (dx * dx + dy * dy).sqrt() as f32;
        let gx = gx.clamp(0.0, (self.hf.width() - 1) as f64);
        let gy = gy.clamp(0.0, (self.hf.height() - 1) as f64);
        (self.hf.sample_bilinear(gx, gy), inside, outside)
    }
}

/// Contrast curve used by the HBAO reference: `gain(x, 3)`.
fn hbao_gain(x: f32, factor: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    if x < 0.5 {
        0.5 * (2.0 * x).powf(factor)
    } else {
        1.0 - 0.5 * (2.0 * (1.0 - x)).powf(factor)
    }
}

/// Procedural star field: one candidate per cell of a spherical grid, with a
/// deterministic sub-cell position and brightness.
fn star_field(dir: Vec3, seed: u64) -> f32 {
    if dir.z <= 0.0 {
        return 0.0;
    }
    let az = dir.y.atan2(dir.x);
    let el = dir.z.clamp(-1.0, 1.0).asin();
    let scale = 70.0;
    let u = az * scale;
    let v = el * scale;
    let iu = u.floor() as i32;
    let iv = v.floor() as i32;
    let mut best = 0.0f32;
    for dj in -1..=1 {
        for di in -1..=1 {
            let cx = iu + di;
            let cy = iv + dj;
            let h = hash01(cx, cy, seed);
            if h < 0.90 {
                continue;
            }
            let h2 = hash01(cx + 7, cy - 3, seed);
            let sx = cx as f32 + 0.15 + 0.7 * h;
            let sy = cy as f32 + 0.15 + 0.7 * h2;
            let d = ((u - sx).powi(2) + (v - sy).powi(2)).sqrt();
            let core = 1.0 - smoothstep(0.0, 0.12, d);
            let bright = 0.30 + 0.70 * h2;
            best = best.max(core * bright);
        }
    }
    best
}

#[inline]
fn hash01(x: i32, y: i32, seed: u64) -> f32 {
    let mut z = seed
        ^ (x as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (y as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    (z >> 40) as f32 * (1.0 / 16_777_216.0)
}
