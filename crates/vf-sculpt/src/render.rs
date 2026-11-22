//! Offscreen CPU viewport: a small, dependency-free rasteriser used by the UI
//! preview, the CLI thumbnails and the documentation renders.
//!
//! It supports a perspective and an orthographic camera, shaded / rock / paint
//! / normal / mask / material / depth modes, a real wireframe overlay, a brush
//! cursor and line overlays. The rasteriser bins triangles per scanline and
//! renders rows in parallel with rayon, so a few hundred thousand triangles
//! stay interactive.

use glam::{Mat4, Vec2, Vec3};
use rayon::prelude::*;

use crate::math::{Aabb, basis_from_normal};
use crate::mesh::{Mesh, RenderMesh};

/// What the viewport shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShadingMode {
    Solid,
    Rock,
    Paint,
    Normals,
    Mask,
    Material,
    Depth,
    AmbientOcclusion,
}

impl ShadingMode {
    pub const ALL: [ShadingMode; 8] = [
        ShadingMode::Solid,
        ShadingMode::Rock,
        ShadingMode::Paint,
        ShadingMode::Normals,
        ShadingMode::Mask,
        ShadingMode::Material,
        ShadingMode::Depth,
        ShadingMode::AmbientOcclusion,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ShadingMode::Solid => "Solide",
            ShadingMode::Rock => "Roche",
            ShadingMode::Paint => "Peinture",
            ShadingMode::Normals => "Normales",
            ShadingMode::Mask => "Masque",
            ShadingMode::Material => "Matériaux",
            ShadingMode::Depth => "Profondeur",
            ShadingMode::AmbientOcclusion => "Occlusion",
        }
    }
}

/// A camera looking at the sculpture.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub fov_y_deg: f32,
    pub ortho: bool,
    /// World height covered vertically in orthographic mode.
    pub ortho_height_m: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            eye: Vec3::new(18.0, -24.0, 14.0),
            target: Vec3::ZERO,
            up: Vec3::Z,
            fov_y_deg: 45.0,
            ortho: false,
            ortho_height_m: 40.0,
            near: 0.05,
            far: 20_000.0,
        }
    }
}

impl Camera {
    /// Orbit around `target` at `distance`, with angles in degrees.
    pub fn orbit(target: Vec3, distance: f32, yaw_deg: f32, pitch_deg: f32) -> Self {
        let yaw = yaw_deg.to_radians();
        let pitch = pitch_deg.clamp(-89.0, 89.0).to_radians();
        let offset = Vec3::new(
            yaw.cos() * pitch.cos(),
            yaw.sin() * pitch.cos(),
            pitch.sin(),
        ) * distance.max(0.1);
        Self {
            eye: target + offset,
            target,
            ..Self::default()
        }
    }

    pub fn forward(&self) -> Vec3 {
        (self.target - self.eye).normalize_or_zero()
    }

    pub fn right(&self) -> Vec3 {
        self.forward().cross(self.up).normalize_or_zero()
    }

    pub fn true_up(&self) -> Vec3 {
        self.right().cross(self.forward()).normalize_or_zero()
    }

    pub fn distance(&self) -> f32 {
        self.eye.distance(self.target)
    }

    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        let view = Mat4::look_at_rh(self.eye, self.target, self.up);
        // A fixed 0.05..20000 range destroys depth precision on a landscape
        // several kilometres wide (adjacent triangles start to z-fight). Keep
        // the ratio around 1:2000 whatever the current framing distance is.
        let distance = self.distance().max(0.1);
        let near = self.near.max(distance * 0.005);
        let far = self.far.min(distance * 20.0 + 500.0).max(near * 20.0);
        if self.ortho {
            let half_h = self.ortho_height_m.max(0.01) * 0.5;
            let half_w = half_h * aspect.max(0.01);
            Mat4::orthographic_rh(-half_w, half_w, -half_h, half_h, near, far) * view
        } else {
            Mat4::perspective_rh(
                self.fov_y_deg.clamp(5.0, 120.0).to_radians(),
                aspect.max(0.01),
                near,
                far,
            ) * view
        }
    }

    /// Project a world point to pixel coordinates and a 0..1 depth.
    pub fn project(&self, point: Vec3, width: u32, height: u32) -> Option<(f32, f32, f32)> {
        if width == 0 || height == 0 {
            return None;
        }
        let clip = self.view_projection(width as f32 / height as f32) * point.extend(1.0);
        if clip.w <= 1e-6 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        if ndc.z < 0.0 || ndc.z > 1.0 {
            return None;
        }
        Some((
            (ndc.x * 0.5 + 0.5) * width as f32,
            (1.0 - (ndc.y * 0.5 + 0.5)) * height as f32,
            ndc.z,
        ))
    }

    /// Ray through a pixel, for picking.
    pub fn ray(&self, x: f32, y: f32, width: u32, height: u32) -> (Vec3, Vec3) {
        let aspect = width as f32 / height.max(1) as f32;
        let ndc_x = (x / width.max(1) as f32) * 2.0 - 1.0;
        let ndc_y = 1.0 - (y / height.max(1) as f32) * 2.0;
        let forward = self.forward();
        let right = self.right();
        let up = self.true_up();
        if self.ortho {
            let half_h = self.ortho_height_m * 0.5;
            let origin = self.eye + right * (ndc_x * half_h * aspect) + up * (ndc_y * half_h);
            (origin, forward)
        } else {
            let tan = (self.fov_y_deg * 0.5).to_radians().tan();
            let direction = forward + right * (ndc_x * tan * aspect) + up * (ndc_y * tan);
            (self.eye, direction.normalize_or_zero())
        }
    }

    /// Frame the camera on a bounding box.
    pub fn frame(mut self, bounds: Aabb) -> Self {
        if bounds.is_empty() {
            return self;
        }
        let center = bounds.center();
        let radius = (bounds.size().length() * 0.5).max(0.2);
        let distance = radius / (self.fov_y_deg * 0.5).to_radians().tan().max(0.05) * 1.4;
        let direction = (self.eye - center).normalize_or_zero();
        let direction = if direction.length_squared() < 0.5 {
            Vec3::new(0.6, -0.7, 0.4).normalize()
        } else {
            direction
        };
        self.target = center;
        self.eye = center + direction * distance;
        self.ortho_height_m = radius * 2.2;
        self
    }
}

/// Render parameters.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RenderSettings {
    pub width: u32,
    pub height: u32,
    pub shading: ShadingMode,
    pub wireframe: bool,
    pub background_top: [u8; 3],
    pub background_bottom: [u8; 3],
    pub sun_direction: Vec3,
    pub sun_color: [f32; 3],
    pub ambient: f32,
    pub fog: f32,
    pub gamma: f32,
    /// Supersampling factor: 1, 2 or 3.
    pub supersample: u32,
    /// Draw the sculpt mask as a red overlay in shaded modes.
    pub mask_overlay: bool,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            shading: ShadingMode::Rock,
            wireframe: false,
            background_top: [86, 122, 160],
            background_bottom: [206, 198, 180],
            sun_direction: Vec3::new(0.45, -0.55, 0.7).normalize(),
            sun_color: [1.0, 0.96, 0.88],
            ambient: 0.42,
            fog: 0.0,
            gamma: 2.2,
            supersample: 1,
            mask_overlay: false,
        }
    }
}

/// A line or circle overlay, in world space.
#[derive(Debug, Clone, Default)]
pub struct Overlay {
    pub lines: Vec<(Vec3, Vec3, [u8; 4])>,
    pub points: Vec<(Vec3, [u8; 4], f32)>,
    /// Translucent filled discs: centre, normal, radius, colour, segments.
    pub discs: Vec<(Vec3, Vec3, f32, [u8; 4], u32)>,
}

impl Overlay {
    pub fn line(&mut self, a: Vec3, b: Vec3, color: [u8; 4]) {
        self.lines.push((a, b, color));
    }

    pub fn point(&mut self, p: Vec3, color: [u8; 4], size: f32) {
        self.points.push((p, color, size));
    }

    /// A translucent disc lying on the surface, used to highlight the brush
    /// footprint before the stroke.
    pub fn disc(&mut self, center: Vec3, normal: Vec3, radius: f32, color: [u8; 4], segments: u32) {
        self.discs
            .push((center, normal, radius, color, segments.max(12)));
    }

    /// An arrow from `origin` along `direction`, with four head wings.
    pub fn arrow(&mut self, origin: Vec3, direction: Vec3, length: f32, color: [u8; 4]) {
        let direction = direction.normalize_or_zero();
        if direction.length_squared() < 0.5 || length <= 0.0 {
            return;
        }
        let tip = origin + direction * length;
        self.line(origin, tip, color);
        let (tangent, bitangent, _) = basis_from_normal(direction);
        let head = length * 0.16;
        for side in [tangent, -tangent, bitangent, -bitangent] {
            self.line(tip, tip - direction * head + side * head * 0.65, color);
        }
    }

    /// Two concentric circles and eight radial ticks: an unambiguous footprint
    /// even when the fill is very transparent.
    pub fn footprint(&mut self, center: Vec3, normal: Vec3, radius: f32, color: [u8; 4]) {
        let (tangent, bitangent, _) = basis_from_normal(normal);
        self.circle(center, normal, radius, color, 48);
        self.circle(
            center,
            normal,
            radius * 0.55,
            [color[0], color[1], color[2], 120],
            32,
        );
        for i in 0..8 {
            let angle = std::f32::consts::TAU * i as f32 / 8.0;
            let dir = tangent * angle.cos() + bitangent * angle.sin();
            self.line(center + dir * radius * 0.88, center + dir * radius, color);
        }
    }

    /// A brush cursor: a circle of `radius` in the surface plane at `center`.
    pub fn circle(
        &mut self,
        center: Vec3,
        normal: Vec3,
        radius: f32,
        color: [u8; 4],
        segments: u32,
    ) {
        let (tangent, bitangent, _) = basis_from_normal(normal);
        let segments = segments.max(8);
        for i in 0..segments {
            let a = std::f32::consts::TAU * i as f32 / segments as f32;
            let b = std::f32::consts::TAU * (i + 1) as f32 / segments as f32;
            let pa = center + (tangent * a.cos() + bitangent * a.sin()) * radius;
            let pb = center + (tangent * b.cos() + bitangent * b.sin()) * radius;
            self.line(pa, pb, color);
        }
    }

    /// A world-space grid, useful as a ground reference.
    pub fn grid(&mut self, extent: f32, step: f32, color: [u8; 4], z: f32) {
        let count = (extent / step).clamp(1.0, 400.0) as i32;
        for i in -count..=count {
            let offset = i as f32 * step;
            self.line(
                Vec3::new(-extent, offset, z),
                Vec3::new(extent, offset, z),
                color,
            );
            self.line(
                Vec3::new(offset, -extent, z),
                Vec3::new(offset, extent, z),
                color,
            );
        }
    }
}

/// Rendered image, RGBA8 row-major from the top.
#[derive(Debug, Clone)]
pub struct RenderImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl RenderImage {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; (width * height * 4) as usize],
        }
    }

    pub fn save(&self, path: &str) -> Result<(), String> {
        let buffer = image::RgbaImage::from_raw(self.width, self.height, self.pixels.clone())
            .ok_or_else(|| "dimensions d'image invalides".to_string())?;
        buffer.save(path).map_err(|error| error.to_string())
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let index = ((y.min(self.height - 1) * self.width + x.min(self.width - 1)) * 4) as usize;
        [
            self.pixels[index],
            self.pixels[index + 1],
            self.pixels[index + 2],
            self.pixels[index + 3],
        ]
    }
}

/// Render a mesh.
pub fn render(mesh: &RenderMesh, camera: &Camera, settings: RenderSettings) -> RenderImage {
    let overlay = Overlay::default();
    render_with_overlay(mesh, camera, settings, &overlay)
}

/// Render a mesh with annotations.
pub fn render_with_overlay(
    mesh: &RenderMesh,
    camera: &Camera,
    settings: RenderSettings,
    overlay: &Overlay,
) -> RenderImage {
    let scale = settings.supersample.clamp(1, 3);
    let width = settings.width.max(16);
    let height = settings.height.max(16);
    let render_width = width * scale;
    let render_height = height * scale;
    let image = rasterize(mesh, camera, settings, render_width, render_height);
    let mut image = if scale == 1 {
        image
    } else {
        downsample(&image, width, height, scale)
    };
    draw_overlay(&mut image, camera, overlay);
    image
}

/// Render a document's mesh directly.
pub fn render_document(
    document: &crate::doc::SculptDocument,
    camera: &Camera,
    settings: RenderSettings,
) -> RenderImage {
    let mesh = document.render_mesh(32.0);
    render(&mesh, camera, settings)
}

fn rasterize(
    mesh: &RenderMesh,
    camera: &Camera,
    settings: RenderSettings,
    width: u32,
    height: u32,
) -> RenderImage {
    let mut image = RenderImage::new(width, height);
    // Sky gradient.
    for y in 0..height {
        let t = y as f32 / height as f32;
        let color = [
            lerp_u8(settings.background_top[0], settings.background_bottom[0], t),
            lerp_u8(settings.background_top[1], settings.background_bottom[1], t),
            lerp_u8(settings.background_top[2], settings.background_bottom[2], t),
            255,
        ];
        for x in 0..width {
            let index = ((y * width + x) * 4) as usize;
            image.pixels[index..index + 4].copy_from_slice(&color);
        }
    }
    if mesh.indices.is_empty() {
        return image;
    }
    let view_projection = camera.view_projection(width as f32 / height as f32);
    let mut screen: Vec<Option<[f32; 3]>> = Vec::with_capacity(mesh.positions.len());
    for position in &mesh.positions {
        let clip = view_projection * position.extend(1.0);
        if clip.w <= 1e-6 {
            screen.push(None);
        } else {
            let inv_w = 1.0 / clip.w;
            screen.push(Some([
                (clip.x * inv_w * 0.5 + 0.5) * width as f32,
                (1.0 - (clip.y * inv_w * 0.5 + 0.5)) * height as f32,
                clip.z * inv_w,
            ]));
        }
    }
    let mut color_buffer = vec![0u8; (width * height * 4) as usize];
    // Start from the same sky gradient as `image`; rows only overwrite the
    // pixels a triangle actually covers.
    for y in 0..height {
        let t = y as f32 / height as f32;
        let sky = [
            lerp_u8(settings.background_top[0], settings.background_bottom[0], t),
            lerp_u8(settings.background_top[1], settings.background_bottom[1], t),
            lerp_u8(settings.background_top[2], settings.background_bottom[2], t),
            255,
        ];
        for x in 0..width {
            let index = ((y * width + x) * 4) as usize;
            color_buffer[index..index + 4].copy_from_slice(&sky);
        }
    }
    // Bin triangles per scanline.
    let mut rows: Vec<Vec<u32>> = vec![Vec::new(); height as usize];
    let mut bboxes: Vec<Option<(u32, u32, u32, u32)>> = Vec::with_capacity(mesh.indices.len());
    for (tri_index, triangle) in mesh.indices.iter().enumerate() {
        let mut points = [[0.0f32; 3]; 3];
        let mut valid = true;
        for (i, &vertex) in triangle.iter().enumerate() {
            match screen[vertex as usize] {
                Some(point) => points[i] = point,
                None => {
                    valid = false;
                    break;
                }
            }
        }
        if !valid {
            bboxes.push(None);
            continue;
        }
        let min_x = points[0][0]
            .min(points[1][0])
            .min(points[2][0])
            .floor()
            .max(0.0) as u32;
        let max_x = points[0][0]
            .max(points[1][0])
            .max(points[2][0])
            .ceil()
            .min((width - 1) as f32) as u32;
        let min_y = points[0][1]
            .min(points[1][1])
            .min(points[2][1])
            .floor()
            .max(0.0) as u32;
        let max_y = points[0][1]
            .max(points[1][1])
            .max(points[2][1])
            .ceil()
            .min((height - 1) as f32) as u32;
        if min_x > max_x || min_y > max_y {
            bboxes.push(None);
            continue;
        }
        bboxes.push(Some((min_x, max_x, min_y, max_y)));
        for y in min_y..=max_y {
            rows[y as usize].push(tri_index as u32);
        }
    }
    // Render rows in parallel.
    let row_colors: Vec<Vec<u8>> = vf_core::compute::install(|| {
        rows.par_iter()
            .enumerate()
            .map(|(y, triangles)| {
                let t = y as f32 / height as f32;
                let sky = [
                    lerp_u8(settings.background_top[0], settings.background_bottom[0], t),
                    lerp_u8(settings.background_top[1], settings.background_bottom[1], t),
                    lerp_u8(settings.background_top[2], settings.background_bottom[2], t),
                    255,
                ];
                let mut row = vec![0u8; (width * 4) as usize];
                for pixel in row.as_chunks_mut::<4>().0 {
                    pixel.copy_from_slice(&sky);
                }
                let mut row_depth = vec![f32::INFINITY; width as usize];
                for &triangle_index in triangles {
                    let triangle = mesh.indices[triangle_index as usize];
                    let Some((min_x, max_x, min_y, max_y)) = bboxes[triangle_index as usize] else {
                        continue;
                    };
                    if y as u32 >= min_y && y as u32 <= max_y {
                        rasterize_triangle_row(
                            y as u32,
                            triangle,
                            &screen,
                            min_x,
                            max_x,
                            mesh,
                            settings,
                            width,
                            &mut row,
                            &mut row_depth,
                        );
                    }
                }
                row
            })
            .collect()
    });
    for (y, row) in row_colors.into_iter().enumerate() {
        let offset = y * width as usize * 4;
        color_buffer[offset..offset + row.len()].copy_from_slice(&row);
    }
    image.pixels.copy_from_slice(&color_buffer);
    if settings.wireframe {
        draw_wireframe(&mut image, mesh, camera, settings);
    }
    image
}

#[allow(clippy::too_many_arguments)]
fn rasterize_triangle_row(
    y: u32,
    triangle: [u32; 3],
    screen: &[Option<[f32; 3]>],
    min_x: u32,
    max_x: u32,
    mesh: &RenderMesh,
    settings: RenderSettings,
    _width: u32,
    row: &mut [u8],
    row_depth: &mut [f32],
) {
    let mut points = [[0.0f32; 3]; 3];
    for (i, &vertex) in triangle.iter().enumerate() {
        let Some(point) = screen[vertex as usize] else {
            return;
        };
        points[i] = point;
    }
    let area = edge(points[0], points[1], points[2]);
    if area.abs() < 1e-9 {
        return;
    }
    let inv_area = 1.0 / area;
    for x in min_x..=max_x {
        let p = [x as f32 + 0.5, y as f32 + 0.5, 0.0];
        let w0 = edge(points[1], points[2], p);
        let w1 = edge(points[2], points[0], p);
        let w2 = edge(points[0], points[1], p);
        if (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0) {
            let bary = [w0 * inv_area, w1 * inv_area, w2 * inv_area];
            let depth = points[0][2] * bary[0] + points[1][2] * bary[1] + points[2][2] * bary[2];
            let pixel = x as usize;
            if depth >= row_depth[pixel] {
                continue;
            }
            row_depth[pixel] = depth;
            let color = shade_pixel(mesh, triangle, bary, depth, settings);
            let index = pixel * 4;
            row[index..index + 4].copy_from_slice(&color);
        }
    }
}

fn edge(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

fn shade_pixel(
    mesh: &RenderMesh,
    triangle: [u32; 3],
    bary: [f32; 3],
    depth: f32,
    settings: RenderSettings,
) -> [u8; 4] {
    let normal = normalize3([
        mesh.normals[triangle[0] as usize].x * bary[0]
            + mesh.normals[triangle[1] as usize].x * bary[1]
            + mesh.normals[triangle[2] as usize].x * bary[2],
        mesh.normals[triangle[0] as usize].y * bary[0]
            + mesh.normals[triangle[1] as usize].y * bary[1]
            + mesh.normals[triangle[2] as usize].y * bary[2],
        mesh.normals[triangle[0] as usize].z * bary[0]
            + mesh.normals[triangle[1] as usize].z * bary[1]
            + mesh.normals[triangle[2] as usize].z * bary[2],
    ]);
    let vertex_color = |vertex: u32| -> [u8; 4] {
        mesh.colors
            .get(vertex as usize)
            .copied()
            .unwrap_or([188, 176, 158, 255])
    };
    let mut base = [
        vertex_color(triangle[0])[0] as f32 * bary[0]
            + vertex_color(triangle[1])[0] as f32 * bary[1]
            + vertex_color(triangle[2])[0] as f32 * bary[2],
        vertex_color(triangle[0])[1] as f32 * bary[0]
            + vertex_color(triangle[1])[1] as f32 * bary[1]
            + vertex_color(triangle[2])[1] as f32 * bary[2],
        vertex_color(triangle[0])[2] as f32 * bary[0]
            + vertex_color(triangle[1])[2] as f32 * bary[1]
            + vertex_color(triangle[2])[2] as f32 * bary[2],
    ];
    // Vertex colours are sRGB; lighting happens in linear space, then the
    // result is encoded back with the configured gamma.
    for channel in &mut base {
        *channel = (*channel / 255.0).powf(2.2);
    }
    let mask = (vertex_color(triangle[0])[3] as f32 * bary[0]
        + vertex_color(triangle[1])[3] as f32 * bary[1]
        + vertex_color(triangle[2])[3] as f32 * bary[2])
        / 255.0;
    match settings.shading {
        ShadingMode::Normals => {
            return [
                (normal[0] * 127.0 + 128.0) as u8,
                (normal[1] * 127.0 + 128.0) as u8,
                (normal[2] * 127.0 + 128.0) as u8,
                255,
            ];
        }
        ShadingMode::Mask => {
            let value = (mask * 255.0) as u8;
            return [value, value, value, 255];
        }
        ShadingMode::Material => {
            let material = triangle[0] / 3;
            let palette = [
                [176u8, 164, 148],
                [148, 158, 166],
                [188, 148, 110],
                [120, 128, 120],
                [196, 188, 172],
            ];
            base = palette[material as usize % palette.len()].map(|v| (v as f32 / 255.0).powf(2.2));
        }
        ShadingMode::Depth => {
            let value = ((1.0 - depth.clamp(0.0, 1.0)) * 255.0) as u8;
            return [value, value, value, 255];
        }
        _ => {}
    }
    let sun = settings.sun_direction.normalize_or_zero();
    let diffuse = (normal[0] * sun.x + normal[1] * sun.y + normal[2] * sun.z).max(0.0);
    let sky = (normal[2] * 0.5 + 0.5).max(0.0);
    let light = settings.ambient * (0.35 + 0.65 * sky) + diffuse * 0.85;
    let warm = settings.sun_color;
    let mut color = [
        base[0] * (light * warm[0] + 0.1),
        base[1] * (light * warm[1] + 0.1),
        base[2] * (light * warm[2] + 0.12),
    ];
    if settings.shading == ShadingMode::Rock || settings.shading == ShadingMode::Solid {
        // A subtle cavity term from the normal's horizontal component makes
        // the strata read without a texture.
        let slope = 1.0 - normal[2].abs();
        let cavity = (1.0 - slope * 0.35).clamp(0.5, 1.0);
        for channel in &mut color {
            *channel *= cavity;
        }
    }
    if settings.mask_overlay && mask < 0.5 {
        color[0] = color[0] * 0.55 + 90.0;
        color[1] *= 0.4;
        color[2] *= 0.4;
    }
    let inv_gamma = 1.0 / settings.gamma.max(0.5);
    [
        (color[0].clamp(0.0, 1.0).powf(inv_gamma) * 255.0) as u8,
        (color[1].clamp(0.0, 1.0).powf(inv_gamma) * 255.0) as u8,
        (color[2].clamp(0.0, 1.0).powf(inv_gamma) * 255.0) as u8,
        255,
    ]
}

fn draw_wireframe(
    image: &mut RenderImage,
    mesh: &RenderMesh,
    camera: &Camera,
    _settings: RenderSettings,
) {
    let mut projected = Vec::with_capacity(mesh.positions.len());
    for position in &mesh.positions {
        projected.push(camera.project(*position, image.width, image.height));
    }
    let color = [24, 26, 28, 190];
    for triangle in &mesh.indices {
        for i in 0..3 {
            let a = projected[triangle[i] as usize];
            let b = projected[triangle[(i + 1) % 3] as usize];
            if let (Some((ax, ay, _)), Some((bx, by, _))) = (a, b) {
                line(image, ax, ay, bx, by, color);
            }
        }
    }
}

/// Bresenham with alpha blending.
fn line(image: &mut RenderImage, x0: f32, y0: f32, x1: f32, y1: f32, color: [u8; 4]) {
    let mut x0 = x0.round() as i32;
    let mut y0 = y0.round() as i32;
    let x1 = x1.round() as i32;
    let y1 = y1.round() as i32;
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut error = dx + dy;
    let width = image.width as i32;
    let height = image.height as i32;
    for _ in 0..(dx.abs() + dy.abs() + 2) {
        if x0 >= 0 && y0 >= 0 && x0 < width && y0 < height {
            let index = ((y0 as u32 * image.width + x0 as u32) * 4) as usize;
            let alpha = color[3] as f32 / 255.0;
            for (channel, pixel) in image.pixels[index..index + 3].iter_mut().enumerate() {
                *pixel = (*pixel as f32 * (1.0 - alpha) + color[channel] as f32 * alpha) as u8;
            }
            image.pixels[index + 3] = 255;
        }
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * error;
        if e2 >= dy {
            error += dy;
            x0 += sx;
        }
        if e2 <= dx {
            error += dx;
            y0 += sy;
        }
    }
}

fn draw_overlay(image: &mut RenderImage, camera: &Camera, overlay: &Overlay) {
    for &(center, normal, radius, color, segments) in &overlay.discs {
        let Some((cx, cy, _)) = camera.project(center, image.width, image.height) else {
            continue;
        };
        let (tangent, bitangent, _) = basis_from_normal(normal);
        let mut ring = Vec::with_capacity(segments as usize);
        let mut visible = true;
        for i in 0..segments {
            let angle = std::f32::consts::TAU * i as f32 / segments as f32;
            let point = center + (tangent * angle.cos() + bitangent * angle.sin()) * radius;
            match camera.project(point, image.width, image.height) {
                Some((x, y, _)) => ring.push((x, y)),
                None => {
                    visible = false;
                    break;
                }
            }
        }
        if visible {
            for i in 0..ring.len() {
                let a = ring[i];
                let b = ring[(i + 1) % ring.len()];
                fill_triangle_alpha(image, (cx, cy), a, b, color);
            }
        }
    }
    for &(a, b, color) in &overlay.lines {
        if let (Some((ax, ay, _)), Some((bx, by, _))) = (
            camera.project(a, image.width, image.height),
            camera.project(b, image.width, image.height),
        ) {
            line(image, ax, ay, bx, by, color);
        }
    }
    for &(point, color, size) in &overlay.points {
        if let Some((x, y, _)) = camera.project(point, image.width, image.height) {
            let radius = size.max(1.0);
            for dy in -(radius as i32)..=(radius as i32) {
                for dx in -(radius as i32)..=(radius as i32) {
                    if (dx * dx + dy * dy) as f32 <= radius * radius {
                        let px = x.round() as i32 + dx;
                        let py = y.round() as i32 + dy;
                        if px >= 0 && py >= 0 && px < image.width as i32 && py < image.height as i32
                        {
                            let index = ((py as u32 * image.width + px as u32) * 4) as usize;
                            image.pixels[index..index + 4].copy_from_slice(&color);
                        }
                    }
                }
            }
        }
    }
}

/// Alpha-blended screen-space triangle, used by the translucent brush disc.
/// The overlay is drawn after the depth pass, so no depth test is needed; the
/// fan is small (a few dozen pixels wide at most).
fn fill_triangle_alpha(
    image: &mut RenderImage,
    a: (f32, f32),
    b: (f32, f32),
    c: (f32, f32),
    color: [u8; 4],
) {
    if color[3] == 0 {
        return;
    }
    let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
    if area.abs() < 1e-6 {
        return;
    }
    let min_x = a.0.min(b.0).min(c.0).floor().max(0.0) as i32;
    let max_x = a.0.max(b.0).max(c.0).ceil().min((image.width - 1) as f32) as i32;
    let min_y = a.1.min(b.1).min(c.1).floor().max(0.0) as i32;
    let max_y = a.1.max(b.1).max(c.1).ceil().min((image.height - 1) as f32) as i32;
    if min_x > max_x || min_y > max_y {
        return;
    }
    let alpha = color[3] as f32 / 255.0;
    let inv_area = 1.0 / area;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let w0 = ((b.0 - px) * (c.1 - py) - (b.1 - py) * (c.0 - px)) * inv_area;
            let w1 = ((c.0 - px) * (a.1 - py) - (c.1 - py) * (a.0 - px)) * inv_area;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let index = ((y as u32 * image.width + x as u32) * 4) as usize;
            for (channel, &value) in color.iter().enumerate().take(3) {
                image.pixels[index + channel] = (image.pixels[index + channel] as f32
                    * (1.0 - alpha)
                    + value as f32 * alpha) as u8;
            }
        }
    }
}

fn downsample(source: &RenderImage, width: u32, height: u32, scale: u32) -> RenderImage {
    let mut out = RenderImage::new(width, height);
    for y in 0..height {
        for x in 0..width {
            let mut sums = [0u32; 4];
            for sy in 0..scale {
                for sx in 0..scale {
                    let pixel = source.pixel(x * scale + sx, y * scale + sy);
                    for (sum, value) in sums.iter_mut().zip(pixel) {
                        *sum += u32::from(value);
                    }
                }
            }
            let index = ((y * width + x) * 4) as usize;
            for (pixel, sum) in out.pixels[index..index + 4].iter_mut().zip(sums) {
                *pixel = (sum / (scale * scale)) as u8;
            }
        }
    }
    out
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)) as u8
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length < 1e-8 {
        [0.0, 0.0, 1.0]
    } else {
        [v[0] / length, v[1] / length, v[2] / length]
    }
}

/// Convenience: render a `Mesh` at the given size.
pub fn thumbnail(mesh: &Mesh, width: u32, height: u32) -> RenderImage {
    let render_mesh = RenderMesh::from_mesh(mesh, 32.0);
    let bounds = mesh.bounds();
    let camera = Camera::default().frame(bounds);
    render(
        &render_mesh,
        &camera,
        RenderSettings {
            width,
            height,
            supersample: 1,
            ..RenderSettings::default()
        },
    )
}

/// The screen-space bounding box of a mesh, for framing and picking tests.
pub fn screen_bounds(
    mesh: &Mesh,
    camera: &Camera,
    width: u32,
    height: u32,
) -> Option<(Vec2, Vec2)> {
    let mut min = Vec2::splat(f32::INFINITY);
    let mut max = Vec2::splat(f32::NEG_INFINITY);
    for &position in &mesh.positions {
        if let Some((x, y, _)) = camera.project(position, width, height) {
            min = min.min(Vec2::new(x, y));
            max = max.max(Vec2::new(x, y));
        }
    }
    if min.x.is_finite() && max.x.is_finite() {
        Some((min, max))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::Mesh;

    fn cube() -> Mesh {
        let positions = vec![
            Vec3::new(-1.0, -1.0, -1.0),
            Vec3::new(1.0, -1.0, -1.0),
            Vec3::new(1.0, 1.0, -1.0),
            Vec3::new(-1.0, 1.0, -1.0),
            Vec3::new(-1.0, -1.0, 1.0),
            Vec3::new(1.0, -1.0, 1.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(-1.0, 1.0, 1.0),
        ];
        let faces = vec![
            vec![0, 3, 2, 1],
            vec![4, 5, 6, 7],
            vec![0, 1, 5, 4],
            vec![1, 2, 6, 5],
            vec![2, 3, 7, 6],
            vec![3, 0, 4, 7],
        ];
        let mut mesh = Mesh::from_faces(positions, &faces);
        mesh.recompute_normals();
        mesh.ensure_colors();
        mesh
    }

    #[test]
    fn a_cube_covers_the_center_of_the_image() {
        let mesh = RenderMesh::from_mesh(&cube(), 30.0);
        let camera = Camera::default().frame(mesh.bounds);
        let settings = RenderSettings {
            width: 320,
            height: 200,
            shading: ShadingMode::Solid,
            background_top: [0, 0, 0],
            background_bottom: [0, 0, 0],
            ..RenderSettings::default()
        };
        let image = render(&mesh, &camera, settings);
        assert_eq!(image.width, 320);
        let center = image.pixel(160, 100);
        assert!(
            center[0] > 0 || center[1] > 0 || center[2] > 0,
            "{center:?}"
        );
        let corner = image.pixel(2, 2);
        assert_eq!(&corner[..3], &[0, 0, 0]);
    }

    #[test]
    fn wireframe_draws_dark_lines() {
        let mesh = RenderMesh::from_mesh(&cube(), 30.0);
        let camera = Camera::default().frame(mesh.bounds);
        let settings = RenderSettings {
            width: 256,
            height: 256,
            wireframe: true,
            ..RenderSettings::default()
        };
        let image = render(&mesh, &camera, settings);
        let dark = image
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] < 60 && p[1] < 60 && p[2] < 60)
            .count();
        assert!(dark > 50, "{dark}");
    }

    #[test]
    fn projection_and_ray_are_consistent() {
        let camera = Camera::orbit(Vec3::ZERO, 20.0, 45.0, 25.0);
        let point = Vec3::new(1.0, -2.0, 3.0);
        let (x, y, _) = camera.project(point, 800, 600).unwrap();
        let (origin, direction) = camera.ray(x, y, 800, 600);
        let to_point = (point - origin).normalize_or_zero();
        assert!(
            direction.dot(to_point) > 0.999,
            "{}",
            direction.dot(to_point)
        );
    }

    #[test]
    fn overlay_circle_is_visible() {
        let mesh = RenderMesh::from_mesh(&cube(), 30.0);
        let camera = Camera::default().frame(mesh.bounds);
        let mut overlay = Overlay::default();
        overlay.circle(Vec3::new(0.0, 0.0, 1.0), Vec3::Z, 0.8, [255, 0, 0, 255], 24);
        let settings = RenderSettings {
            width: 200,
            height: 200,
            ..RenderSettings::default()
        };
        let image = render_with_overlay(&mesh, &camera, settings, &overlay);
        let red = image
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] > 180 && p[1] < 80)
            .count();
        assert!(red > 20, "{red}");
    }

    #[test]
    fn overlay_disc_and_arrow_change_pixels() {
        let mesh = RenderMesh::from_mesh(&cube(), 30.0);
        let camera = Camera::default().frame(mesh.bounds);
        let settings = RenderSettings {
            width: 200,
            height: 200,
            ..RenderSettings::default()
        };
        let base = render_with_overlay(&mesh, &camera, settings, &Overlay::default());
        let mut overlay = Overlay::default();
        overlay.disc(
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::Z,
            0.7,
            [255, 160, 40, 150],
            32,
        );
        overlay.footprint(Vec3::new(0.0, 0.0, 1.0), Vec3::Z, 0.7, [255, 220, 120, 255]);
        overlay.arrow(Vec3::new(0.0, 0.0, 1.0), Vec3::Z, 1.2, [150, 228, 255, 255]);
        let image = render_with_overlay(&mesh, &camera, settings, &overlay);
        let changed = base
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .zip(image.pixels.as_chunks::<4>().0)
            .filter(|(before, after)| {
                (0..3).any(|channel| before[channel].abs_diff(after[channel]) > 6)
            })
            .count();
        assert!(changed > 200, "{changed}");
    }

    #[test]
    fn orthographic_camera_projects_parallel_rays() {
        let camera = Camera {
            ortho: true,
            ortho_height_m: 10.0,
            ..Camera::orbit(Vec3::ZERO, 30.0, 0.0, 30.0)
        };
        let a = camera.ray(10.0, 10.0, 100, 100);
        let b = camera.ray(90.0, 90.0, 100, 100);
        assert!((a.1 - b.1).length() < 1e-5);
    }
}
