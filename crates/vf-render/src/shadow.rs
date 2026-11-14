//! Sun shadow map: an orthographic depth map rendered from the light.

use glam::{Mat4, Vec3, Vec4Swizzles};

use crate::{KIND_TERRAIN, Tri, Vertex};

pub struct ShadowMap {
    size: usize,
    depths: Vec<f32>,
    matrix: Mat4,
    /// World-space normal offset used to fight acne, in metres.
    normal_bias: f32,
    depth_bias: f32,
}

impl ShadowMap {
    pub fn size(&self) -> usize {
        self.size
    }

    /// Percentage-closer filtered visibility in `[0, 1]` (1 = lit).
    pub fn sample(&self, pos_abs: Vec3, normal: Vec3, light_dir: Vec3, softness: f32) -> f32 {
        let p = pos_abs + normal * self.normal_bias + light_dir * self.normal_bias;
        let clip = self.matrix * p.extend(1.0);
        if clip.w.abs() < 1e-9 {
            return 1.0;
        }
        let ndc = clip.xyz() / clip.w;
        let u = ndc.x * 0.5 + 0.5;
        let v = (1.0 - (ndc.y * 0.5 + 0.5)).clamp(0.0, 1.0);
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return 1.0;
        }
        // Normal-dependent bias: a surface lit edge-on needs a much larger
        // bias than one facing the sun. This is the same trick as the
        // reference renderer and removes most acne without peter-panning.
        let ndl = normal.dot(light_dir).clamp(0.0, 1.0);
        let bias = self.depth_bias * (0.4 + 1.8 * (1.0 - ndl));
        let depth = ndc.z - bias;
        let radius = softness.clamp(0.0, 3.0);
        let texel = 1.0 / self.size as f32;
        // 5x5 PCF with radial weights: smoother contact shadows than the
        // previous 3x3 box, for a negligible cost at 4096.
        let mut lit = 0.0f32;
        let mut taps = 0.0f32;
        for oy in -2i32..=2 {
            for ox in -2i32..=2 {
                let su = u + ox as f32 * texel * radius;
                let sv = v + oy as f32 * texel * radius;
                let weight = 1.0 - (ox * ox + oy * oy) as f32 * 0.05;
                if !(0.0..=1.0).contains(&su) || !(0.0..=1.0).contains(&sv) {
                    lit += weight;
                } else {
                    let x = (su * (self.size - 1) as f32).round() as usize;
                    let y = (sv * (self.size - 1) as f32).round() as usize;
                    let stored = self.depths[y * self.size + x];
                    lit += if stored + bias >= depth { weight } else { 0.0 };
                }
                taps += weight;
            }
        }
        if taps > 0.0 { lit / taps } else { 1.0 }
    }
}

pub(crate) fn build(
    vertices: &[Vertex],
    triangles: &[Tri],
    light_dir: Vec3,
    center_abs: Vec3,
    extent: f32,
    size: usize,
) -> ShadowMap {
    let size = size.clamp(256, 8192);
    let up = if light_dir.z.abs() > 0.99 {
        Vec3::Y
    } else {
        Vec3::Z
    };
    let distance = extent * 3.0 + 100.0;
    let eye = center_abs + light_dir * distance;
    let view = Mat4::look_at_rh(eye, center_abs, up);
    let half = extent * 0.95;
    let proj = Mat4::orthographic_rh(-half, half, -half, half, 0.1, distance * 2.0 + extent);
    let matrix = proj * view;
    let mut depths = vec![1.0f32; size * size];

    for tri in triangles.iter().filter(|t| t.kind == KIND_TERRAIN) {
        let v0 = vertices[tri.a as usize];
        let v1 = vertices[tri.b as usize];
        let v2 = vertices[tri.c as usize];
        let p0 = matrix * (v0.pos + center_abs).extend(1.0);
        let p1 = matrix * (v1.pos + center_abs).extend(1.0);
        let p2 = matrix * (v2.pos + center_abs).extend(1.0);
        if p0.w.abs() < 1e-9 || p1.w.abs() < 1e-9 || p2.w.abs() < 1e-9 {
            continue;
        }
        let n0 = p0.xyz() / p0.w;
        let n1 = p1.xyz() / p1.w;
        let n2 = p2.xyz() / p2.w;
        if n0.z < 0.0 && n1.z < 0.0 && n2.z < 0.0 {
            continue;
        }
        if n0.z > 1.0 && n1.z > 1.0 && n2.z > 1.0 {
            continue;
        }
        let sx0 = (n0.x * 0.5 + 0.5) * size as f32;
        let sy0 = (1.0 - (n0.y * 0.5 + 0.5)) * size as f32;
        let sx1 = (n1.x * 0.5 + 0.5) * size as f32;
        let sy1 = (1.0 - (n1.y * 0.5 + 0.5)) * size as f32;
        let sx2 = (n2.x * 0.5 + 0.5) * size as f32;
        let sy2 = (1.0 - (n2.y * 0.5 + 0.5)) * size as f32;
        let (ax, ay, az, mut bx, mut by, mut bz, mut cx, mut cy, mut cz) =
            (sx0, sy0, n0.z, sx1, sy1, n1.z, sx2, sy2, n2.z);
        let area = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
        if area.abs() < 1e-9 {
            continue;
        }
        if area < 0.0 {
            std::mem::swap(&mut bx, &mut cx);
            std::mem::swap(&mut by, &mut cy);
            std::mem::swap(&mut bz, &mut cz);
        }
        let min_x = ax.min(bx).min(cx).floor().max(0.0) as usize;
        let max_x = ax.max(bx).max(cx).ceil().min(size as f32) as usize;
        let min_y = ay.min(by).min(cy).floor().max(0.0) as usize;
        let max_y = ay.max(by).max(cy).ceil().min(size as f32) as usize;
        if min_x >= max_x || min_y >= max_y {
            continue;
        }
        for y in min_y..max_y {
            let py = y as f32 + 0.5;
            for x in min_x..max_x {
                let px = x as f32 + 0.5;
                let w0 = (bx - px) * (cy - py) - (by - py) * (cx - px);
                let w1 = (cx - px) * (ay - py) - (cy - py) * (ax - px);
                let w2 = (ax - px) * (by - py) - (ay - py) * (bx - px);
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let sum = w0 + w1 + w2;
                if sum <= 1e-9 {
                    continue;
                }
                let depth = (w0 * az + w1 * bz + w2 * cz) / sum;
                if depth < 0.0 || depth > 1.0 {
                    continue;
                }
                let idx = y * size + x;
                if depth < depths[idx] {
                    depths[idx] = depth;
                }
            }
        }
    }

    ShadowMap {
        size,
        depths,
        matrix,
        normal_bias: (extent * 0.0012).max(0.01),
        depth_bias: 0.0015,
    }
}
