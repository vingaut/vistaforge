//! Small geometric primitives shared by the sculpting modules.
//!
//! Everything is plain `glam` arithmetic: no external acceleration library, so
//! the module compiles on every platform VistaForge targets and stays
//! deterministic (single-threaded reductions are never reordered by a
//! parallel primitive).

use glam::{Vec2, Vec3};

/// Axis-aligned bounding box. An empty box has `min > max` on every axis.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Default for Aabb {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl Aabb {
    pub const EMPTY: Self = Self {
        min: Vec3::splat(f32::INFINITY),
        max: Vec3::splat(f32::NEG_INFINITY),
    };

    pub fn new(min: Vec3, max: Vec3) -> Self {
        Self { min, max }
    }

    pub fn from_points(points: &[Vec3]) -> Self {
        let mut out = Self::EMPTY;
        for &p in points {
            out = out.with_point(p);
        }
        out
    }

    /// Axis-aligned box centred on `center` with the given full size.
    pub fn from_center_size(center: Vec3, size: Vec3) -> Self {
        let half = size.abs() * 0.5;
        Self::new(center - half, center + half)
    }

    pub fn with_point(self, p: Vec3) -> Self {
        Self {
            min: self.min.min(p),
            max: self.max.max(p),
        }
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    pub fn expanded(self, margin: f32) -> Self {
        Self {
            min: self.min - Vec3::splat(margin),
            max: self.max + Vec3::splat(margin),
        }
    }

    pub fn intersected(self, other: Self) -> Self {
        Self {
            min: self.min.max(other.min),
            max: self.max.min(other.max),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    pub fn diagonal(&self) -> f32 {
        self.size().length()
    }

    pub fn longest_axis(&self) -> usize {
        let s = self.size();
        if s.x >= s.y && s.x >= s.z {
            0
        } else if s.y >= s.z {
            1
        } else {
            2
        }
    }

    pub fn contains(&self, p: Vec3) -> bool {
        p.cmpge(self.min).all() && p.cmple(self.max).all()
    }

    /// Squared distance from a point to the box (zero inside).
    pub fn distance_sq(&self, p: Vec3) -> f32 {
        if self.is_empty() {
            return f32::INFINITY;
        }
        let d = (self.min - p).max(p - self.max).max(Vec3::ZERO);
        d.length_squared()
    }

    pub fn intersects(&self, other: &Self) -> bool {
        !(self.max.x < other.min.x
            || self.min.x > other.max.x
            || self.max.y < other.min.y
            || self.min.y > other.max.y
            || self.max.z < other.min.z
            || self.min.z > other.max.z)
    }

    /// Ray entry distance, or `None` when the ray misses. Used by the BVH
    /// slab tests; a direction component of zero is handled with infinities.
    pub fn ray_hit(&self, origin: Vec3, inv_dir: Vec3, t_max: f32) -> Option<(f32, f32)> {
        let mut t0 = 0.0f32;
        let mut t1 = t_max;
        for axis in 0..3 {
            let o = origin[axis];
            let inv = inv_dir[axis];
            let mut near = (self.min[axis] - o) * inv;
            let mut far = (self.max[axis] - o) * inv;
            if near.is_nan() || far.is_nan() {
                // Parallel ray exactly on a slab boundary: treat as hit only
                // when the origin is already inside the slab.
                if o < self.min[axis] || o > self.max[axis] {
                    return None;
                }
                near = f32::NEG_INFINITY;
                far = f32::INFINITY;
            }
            if near > far {
                core::mem::swap(&mut near, &mut far);
            }
            t0 = t0.max(near);
            t1 = t1.min(far);
            if t0 > t1 {
                return None;
            }
        }
        Some((t0, t1))
    }
}

/// A ray with a normalised direction.
#[derive(Debug, Clone, Copy)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Ray {
    pub fn new(origin: Vec3, dir: Vec3) -> Self {
        Self {
            origin,
            dir: dir.normalize_or_zero(),
        }
    }

    pub fn at(&self, t: f32) -> Vec3 {
        self.origin + self.dir * t
    }
}

/// Closest point on a triangle, with barycentric coordinates.
pub fn closest_point_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> (Vec3, Vec3) {
    // Ericson, Real-Time Collision Detection, §5.1.5.
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, Vec3::new(1.0, 0.0, 0.0));
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return (b, Vec3::new(0.0, 1.0, 0.0));
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (a + ab * v, Vec3::new(1.0 - v, v, 0.0));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return (c, Vec3::new(0.0, 0.0, 1.0));
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (a + ac * w, Vec3::new(1.0 - w, 0.0, w));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (b + (c - b) * w, Vec3::new(0.0, 1.0 - w, w));
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    (a + ab * v + ac * w, Vec3::new(1.0 - v - w, v, w))
}

/// Möller–Trumbore ray/triangle intersection. Returns `(t, u, v)`.
pub fn ray_triangle(ray: &Ray, a: Vec3, b: Vec3, c: Vec3) -> Option<(f32, f32, f32)> {
    const EPS: f32 = 1e-8;
    let e1 = b - a;
    let e2 = c - a;
    let p = ray.dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < EPS {
        return None;
    }
    let inv_det = 1.0 / det;
    let t_vec = ray.origin - a;
    let u = t_vec.dot(p) * inv_det;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = t_vec.cross(e1);
    let v = ray.dir.dot(q) * inv_det;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv_det;
    if t > EPS { Some((t, u, v)) } else { None }
}

/// Smooth Hermite interpolation between two edges.
pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if (edge1 - edge0).abs() < f32::EPSILON {
        return if x >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Falloff curves offered by every brush.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Falloff {
    Smooth,
    Linear,
    Constant,
    Sharp,
    Root,
    Sphere,
    Plateau,
}

impl Falloff {
    pub const ALL: [Falloff; 7] = [
        Falloff::Smooth,
        Falloff::Linear,
        Falloff::Constant,
        Falloff::Sharp,
        Falloff::Root,
        Falloff::Sphere,
        Falloff::Plateau,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Falloff::Smooth => "Lisse",
            Falloff::Linear => "Linéaire",
            Falloff::Constant => "Constant",
            Falloff::Sharp => "Net",
            Falloff::Root => "Racine",
            Falloff::Sphere => "Sphère",
            Falloff::Plateau => "Plateau",
        }
    }

    /// `t` is the normalised distance from the brush centre: 0 at the centre,
    /// 1 at the radius. Returns a weight in `[0, 1]`.
    pub fn weight(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Falloff::Smooth => {
                let s = 1.0 - t;
                s * s * (3.0 - 2.0 * s)
            }
            Falloff::Linear => 1.0 - t,
            Falloff::Constant => 1.0,
            Falloff::Sharp => (1.0 - t).powi(3),
            Falloff::Root => (1.0 - t).sqrt(),
            Falloff::Sphere => (1.0 - t * t).max(0.0).sqrt(),
            Falloff::Plateau => smoothstep(1.0, 0.55, t),
        }
    }
}

/// Orthonormal basis around `n`, deterministic (no random orientation).
pub fn basis_from_normal(n: Vec3) -> (Vec3, Vec3, Vec3) {
    let n = n.normalize_or_zero();
    let n = if n.length_squared() < 0.5 { Vec3::Z } else { n };
    let helper = if n.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let t = helper.cross(n).normalize_or_zero();
    let b = n.cross(t);
    (t, b, n)
}

/// Deterministic 64-bit hash used by every random choice of the module.
/// Same seed + same index always gives the same value, on every platform.
#[inline]
pub fn hash_u64(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ceb9fe1a85ec53);
    x ^= x >> 33;
    x
}

#[inline]
pub fn hash01(seed: u64, a: u64) -> f32 {
    (hash_u64(seed ^ a.wrapping_mul(0x9e3779b97f4a7c15)) >> 40) as f32 / (1u64 << 24) as f32
}

#[inline]
pub fn hash2(seed: u64, x: u64, y: u64) -> f32 {
    hash01(seed, x ^ hash_u64(y.wrapping_add(0x632be59bd9b4e019)))
}

/// Uniform random floats on the unit sphere (deterministic).
pub fn random_unit_vector(seed: u64, index: u64) -> Vec3 {
    let a = hash01(seed, index.wrapping_mul(2)) * std::f32::consts::TAU;
    let z = hash01(seed, index.wrapping_mul(2) + 1) * 2.0 - 1.0;
    let r = (1.0 - z * z).max(0.0).sqrt();
    Vec3::new(r * a.cos(), r * a.sin(), z)
}

/// Distance from a point to a segment, plus the parameter along the segment.
pub fn point_segment_distance(p: Vec3, a: Vec3, b: Vec3) -> (f32, f32) {
    let ab = b - a;
    let len_sq = ab.length_squared();
    if len_sq < 1e-12 {
        return ((p - a).length(), 0.0);
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    ((p - (a + ab * t)).length(), t)
}

/// Project a point on the plane defined by `origin` and `normal` to 2D.
pub fn plane_basis(normal: Vec3) -> (Vec3, Vec3) {
    let (t, b, _) = basis_from_normal(normal);
    (t, b)
}

/// 2D points of a plane coordinate system, ordered counter-clockwise.
pub fn signed_area_2d(points: &[Vec2]) -> f32 {
    let mut area = 0.0;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        area += a.x * b.y - b.x * a.y;
    }
    area * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closest_point_matches_vertices_and_edges() {
        let a = Vec3::new(0.0, 0.0, 0.0);
        let b = Vec3::new(1.0, 0.0, 0.0);
        let c = Vec3::new(0.0, 1.0, 0.0);
        assert!((closest_point_triangle(a, a, b, c).0 - a).length() < 1e-6);
        assert!(
            (closest_point_triangle(Vec3::new(0.5, 0.5, 0.0), a, b, c).0
                - Vec3::new(0.5, 0.5, 0.0))
            .length()
                < 1e-6
        );
        assert!((closest_point_triangle(Vec3::new(2.0, -1.0, 0.0), a, b, c).0 - b).length() < 1e-6);
    }

    #[test]
    fn falloffs_are_centered_and_bounded() {
        for falloff in Falloff::ALL {
            assert!((falloff.weight(0.0) - 1.0).abs() < 1e-5, "{falloff:?}");
            if falloff != Falloff::Constant {
                assert!(falloff.weight(1.0).abs() < 1e-5, "{falloff:?}");
            }
            assert!((0.0..=1.0).contains(&falloff.weight(0.4)));
        }
    }

    #[test]
    fn hash_is_deterministic_and_in_range() {
        for i in 0..100 {
            let a = hash01(7, i);
            assert_eq!(a, hash01(7, i));
            assert!((0.0..1.0).contains(&a));
        }
        let v = random_unit_vector(3, 9);
        assert!((v.length() - 1.0).abs() < 1e-4);
    }
}
