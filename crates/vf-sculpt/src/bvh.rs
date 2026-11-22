//! Bounding volume hierarchy over mesh triangles.
//!
//! The BVH is the workhorse of the sculpting module: every brush query, every
//! ground-clearance lookup and every voxel sample goes through it. It stores
//! the triangle soup (a fan triangulation of the mesh) once, and rebuilds in
//! O(n log n) after a topology change. Vertex-only edits are handled by
//! `refit`, which walks the tree bottom-up instead of rebuilding it, so a
//! stroke stays interactive.
//!
//! The tree is built with a binned surface-area heuristic and traversed
//! nearest-child-first. Queries take the mesh explicitly so that the BVH never
//! copies positions, and a refit is always consistent with the current mesh.

use glam::Vec3;
use rayon::prelude::*;

use crate::math::{Aabb, Ray, closest_point_triangle, ray_triangle};
use crate::mesh::Mesh;

const LEAF_SIZE: usize = 4;
const SAH_BINS: usize = 12;
const MAX_DEPTH: u32 = 48;

#[derive(Debug, Clone, Copy)]
struct Node {
    bounds: Aabb,
    /// Left child for an internal node, first triangle for a leaf.
    left_or_start: u32,
    /// Right child for an internal node, triangle count for a leaf.
    right_or_count: u32,
    leaf: bool,
}

impl Node {
    fn leaf(bounds: Aabb, start: u32, count: u32) -> Self {
        Self {
            bounds,
            left_or_start: start,
            right_or_count: count,
            leaf: true,
        }
    }

    fn internal(bounds: Aabb, left: u32, right: u32) -> Self {
        Self {
            bounds,
            left_or_start: left,
            right_or_count: right,
            leaf: false,
        }
    }
}

/// A hit from a ray query.
#[derive(Debug, Clone, Copy)]
pub struct RayHit {
    pub t: f32,
    pub triangle_index: u32,
    pub triangle: [u32; 3],
    /// Barycentric coordinates `(1 - u - v, u, v)`.
    pub barycentric: Vec3,
    /// Geometric (flat) normal, normalised.
    pub normal: Vec3,
    pub position: Vec3,
}

/// A hit from a closest-point query.
#[derive(Debug, Clone, Copy)]
pub struct ClosestHit {
    pub distance_sq: f32,
    pub triangle_index: u32,
    pub triangle: [u32; 3],
    pub barycentric: Vec3,
    /// Interpolated smooth normal when the mesh has one, geometric otherwise.
    pub normal: Vec3,
    pub position: Vec3,
}

#[derive(Debug, Clone, Default)]
pub struct Bvh {
    nodes: Vec<Node>,
    /// Triangle indices in leaf order.
    order: Vec<u32>,
    /// Fan triangulation of the mesh faces, as vertex indices.
    triangles: Vec<[u32; 3]>,
    /// Cached triangle centroids, rebuilt with the tree.
    centroids: Vec<Vec3>,
    bounds: Aabb,
}

impl Bvh {
    pub fn build(mesh: &Mesh) -> Self {
        let triangles = mesh.triangulated_indices();
        // Centroids and per-triangle boxes are pure functions of the positions:
        // compute them once on the shared pool instead of once per tree level.
        // The recursion below then only unions cached boxes, which is both
        // faster (no repeated min/max over the three corners) and the bulk of
        // the build time on a large mesh.
        let mut centroids = vec![Vec3::ZERO; triangles.len()];
        let mut boxes = vec![Aabb::EMPTY; triangles.len()];
        let positions = &mesh.positions;
        vf_core::compute::install(|| {
            centroids
                .par_iter_mut()
                .zip(boxes.par_iter_mut())
                .zip(triangles.par_iter())
                .for_each(|((centroid, bounds), tri)| {
                    let a = positions[tri[0] as usize];
                    let b = positions[tri[1] as usize];
                    let c = positions[tri[2] as usize];
                    *centroid = (a + b + c) / 3.0;
                    *bounds = Aabb {
                        min: a.min(b).min(c),
                        max: a.max(b).max(c),
                    };
                });
        });
        let mut bvh = Self {
            nodes: Vec::with_capacity(triangles.len().max(1) * 2),
            order: (0..triangles.len() as u32).collect(),
            centroids,
            triangles,
            bounds: Aabb::EMPTY,
        };
        if bvh.triangles.is_empty() {
            bvh.nodes.push(Node::leaf(Aabb::EMPTY, 0, 0));
            return bvh;
        }
        bvh.build_range(&boxes, 0, bvh.order.len(), 0);
        bvh.bounds = bvh.nodes[0].bounds;
        bvh
    }

    fn triangle_bounds(&self, mesh: &Mesh, index: u32) -> Aabb {
        let tri = self.triangles[index as usize];
        let a = mesh.positions[tri[0] as usize];
        let b = mesh.positions[tri[1] as usize];
        let c = mesh.positions[tri[2] as usize];
        Aabb {
            min: a.min(b).min(c),
            max: a.max(b).max(c),
        }
    }

    fn build_range(&mut self, boxes: &[Aabb], start: usize, end: usize, depth: u32) -> u32 {
        let node_index = self.nodes.len() as u32;
        self.nodes.push(Node::leaf(Aabb::EMPTY, 0, 0));
        let count = end - start;
        let mut bounds = Aabb::EMPTY;
        let mut centroid_bounds = Aabb::EMPTY;
        for i in start..end {
            let tri = self.order[i];
            bounds = bounds.union(boxes[tri as usize]);
            centroid_bounds = centroid_bounds.with_point(self.centroids[tri as usize]);
        }
        if count <= LEAF_SIZE || depth >= MAX_DEPTH || centroid_bounds.diagonal() <= 1e-7 {
            self.nodes[node_index as usize] = Node::leaf(bounds, start as u32, count as u32);
            return node_index;
        }

        // Binned SAH: 12 bins on each axis, pick the cheapest split.
        let axis = centroid_bounds.longest_axis();
        let extent = centroid_bounds.size()[axis].max(1e-6);
        let scale = SAH_BINS as f32 / extent;
        let mut bin_bounds = [Aabb::EMPTY; SAH_BINS];
        let mut bin_counts = [0u32; SAH_BINS];
        for i in start..end {
            let tri = self.order[i];
            let c = self.centroids[tri as usize][axis];
            let bin = (((c - centroid_bounds.min[axis]) * scale) as usize).min(SAH_BINS - 1);
            bin_bounds[bin] = bin_bounds[bin].union(boxes[tri as usize]);
            bin_counts[bin] += 1;
        }
        let mut left_area = [0.0f32; SAH_BINS];
        let mut left_count = [0u32; SAH_BINS];
        let mut acc_bounds = Aabb::EMPTY;
        let mut acc_count = 0;
        for i in 0..SAH_BINS {
            acc_bounds = acc_bounds.union(bin_bounds[i]);
            acc_count += bin_counts[i];
            left_area[i] = area_of(acc_bounds);
            left_count[i] = acc_count;
        }
        let mut right_area = [0.0f32; SAH_BINS];
        let mut right_count = [0u32; SAH_BINS];
        let mut acc_bounds = Aabb::EMPTY;
        let mut acc_count = 0;
        for i in (0..SAH_BINS).rev() {
            acc_bounds = acc_bounds.union(bin_bounds[i]);
            acc_count += bin_counts[i];
            right_area[i] = area_of(acc_bounds);
            right_count[i] = acc_count;
        }
        let mut best_cost = f32::INFINITY;
        let mut best_split = SAH_BINS / 2;
        for i in 0..SAH_BINS - 1 {
            let l = left_count[i];
            if l == 0 || l == count as u32 {
                continue;
            }
            let cost = left_area[i] * l as f32 + right_area[i + 1] * right_count[i + 1] as f32;
            if cost < best_cost {
                best_cost = cost;
                best_split = i + 1;
            }
        }
        let mut mid = start;
        for i in start..end {
            let tri = self.order[i];
            let c = self.centroids[tri as usize][axis];
            let bin = (((c - centroid_bounds.min[axis]) * scale) as usize).min(SAH_BINS - 1);
            if bin < best_split {
                self.order.swap(mid, i);
                mid += 1;
            }
        }
        if mid == start || mid == end {
            // Degenerate distribution: fall back to a median split.
            mid = start + count / 2;
            let (left, right) = self.order[start..end].split_at_mut(count / 2);
            left.sort_unstable();
            right.sort_unstable();
        }
        let left = self.build_range(boxes, start, mid, depth + 1);
        let right = self.build_range(boxes, mid, end, depth + 1);
        self.nodes[node_index as usize] = Node::internal(bounds, left, right);
        node_index
    }

    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn bounds(&self) -> Aabb {
        self.bounds
    }

    pub fn triangles(&self) -> &[[u32; 3]] {
        &self.triangles
    }

    /// Recompute every node bound from the current vertex positions. Called
    /// after a stroke moved vertices; topology is unchanged.
    pub fn refit(&mut self, mesh: &Mesh) {
        if self.triangles.is_empty() {
            return;
        }
        self.bounds = self.refit_node(mesh, 0);
    }

    fn refit_node(&mut self, mesh: &Mesh, index: u32) -> Aabb {
        let (leaf, a, b) = {
            let node = self.nodes[index as usize];
            (node.leaf, node.left_or_start, node.right_or_count)
        };
        if leaf {
            let mut bounds = Aabb::EMPTY;
            for i in a..a + b {
                let tri = self.order[i as usize];
                bounds = bounds.union(self.triangle_bounds(mesh, tri));
            }
            self.nodes[index as usize].bounds = bounds;
            bounds
        } else {
            let left = self.refit_node(mesh, a);
            let right = self.refit_node(mesh, b);
            let bounds = left.union(right);
            self.nodes[index as usize].bounds = bounds;
            bounds
        }
    }

    pub fn raycast(&self, mesh: &Mesh, ray: &Ray, t_min: f32, t_max: f32) -> Option<RayHit> {
        if self.triangles.is_empty() {
            return None;
        }
        let inv_dir = Vec3::new(
            if ray.dir.x.abs() < 1e-12 {
                f32::INFINITY
            } else {
                1.0 / ray.dir.x
            },
            if ray.dir.y.abs() < 1e-12 {
                f32::INFINITY
            } else {
                1.0 / ray.dir.y
            },
            if ray.dir.z.abs() < 1e-12 {
                f32::INFINITY
            } else {
                1.0 / ray.dir.z
            },
        );
        let mut best: Option<RayHit> = None;
        let mut best_t = t_max;
        let mut stack = Vec::with_capacity(64);
        stack.push(0u32);
        while let Some(node_index) = stack.pop() {
            let node = self.nodes[node_index as usize];
            if node.bounds.is_empty() {
                continue;
            }
            let Some((entry, _)) = node.bounds.ray_hit(ray.origin, inv_dir, best_t) else {
                continue;
            };
            if entry > best_t {
                continue;
            }
            if node.leaf {
                for i in node.left_or_start..node.left_or_start + node.right_or_count {
                    let tri_index = self.order[i as usize];
                    let tri = self.triangles[tri_index as usize];
                    let a = mesh.positions[tri[0] as usize];
                    let b = mesh.positions[tri[1] as usize];
                    let c = mesh.positions[tri[2] as usize];
                    if let Some((t, u, v)) = ray_triangle(ray, a, b, c)
                        && t >= t_min
                        && t < best_t
                    {
                        best_t = t;
                        let position = ray.at(t);
                        let normal = (b - a).cross(c - a).normalize_or_zero();
                        best = Some(RayHit {
                            t,
                            triangle_index: tri_index,
                            triangle: tri,
                            barycentric: Vec3::new(1.0 - u - v, u, v),
                            normal,
                            position,
                        });
                    }
                }
            } else {
                // Visit the nearer child last so it is popped first.
                let left = &self.nodes[node.left_or_start as usize];
                let right = &self.nodes[node.right_or_count as usize];
                let dl = left
                    .bounds
                    .ray_hit(ray.origin, inv_dir, best_t)
                    .map(|(t, _)| t)
                    .unwrap_or(f32::INFINITY);
                let dr = right
                    .bounds
                    .ray_hit(ray.origin, inv_dir, best_t)
                    .map(|(t, _)| t)
                    .unwrap_or(f32::INFINITY);
                if dl <= dr {
                    if dr.is_finite() {
                        stack.push(node.right_or_count);
                    }
                    if dl.is_finite() {
                        stack.push(node.left_or_start);
                    }
                } else {
                    if dl.is_finite() {
                        stack.push(node.left_or_start);
                    }
                    if dr.is_finite() {
                        stack.push(node.right_or_count);
                    }
                }
            }
        }
        best
    }

    /// Closest point on the surface of `mesh` to `p`.
    pub fn closest_point(&self, mesh: &Mesh, p: Vec3) -> Option<ClosestHit> {
        if self.triangles.is_empty() {
            return None;
        }
        let mut best: Option<ClosestHit> = None;
        let mut best_sq = f32::INFINITY;
        let mut stack = Vec::with_capacity(64);
        stack.push(0u32);
        while let Some(node_index) = stack.pop() {
            let node = self.nodes[node_index as usize];
            if node.bounds.distance_sq(p) >= best_sq {
                continue;
            }
            if node.leaf {
                for i in node.left_or_start..node.left_or_start + node.right_or_count {
                    let tri_index = self.order[i as usize];
                    let tri = self.triangles[tri_index as usize];
                    let a = mesh.positions[tri[0] as usize];
                    let b = mesh.positions[tri[1] as usize];
                    let c = mesh.positions[tri[2] as usize];
                    let (point, bary) = closest_point_triangle(p, a, b, c);
                    let distance_sq = point.distance_squared(p);
                    if distance_sq < best_sq {
                        best_sq = distance_sq;
                        let geometric = (b - a).cross(c - a).normalize_or_zero();
                        let normal = if mesh.normals.len() == mesh.positions.len() {
                            let n = mesh.normals[tri[0] as usize] * bary.x
                                + mesh.normals[tri[1] as usize] * bary.y
                                + mesh.normals[tri[2] as usize] * bary.z;
                            n.normalize_or_zero()
                        } else {
                            geometric
                        };
                        best = Some(ClosestHit {
                            distance_sq,
                            triangle_index: tri_index,
                            triangle: tri,
                            barycentric: bary,
                            normal,
                            position: point,
                        });
                    }
                }
            } else {
                let left = &self.nodes[node.left_or_start as usize];
                let right = &self.nodes[node.right_or_count as usize];
                let dl = left.bounds.distance_sq(p);
                let dr = right.bounds.distance_sq(p);
                if dl <= dr {
                    if dr < best_sq {
                        stack.push(node.right_or_count);
                    }
                    if dl < best_sq {
                        stack.push(node.left_or_start);
                    }
                } else {
                    if dl < best_sq {
                        stack.push(node.left_or_start);
                    }
                    if dr < best_sq {
                        stack.push(node.right_or_count);
                    }
                }
            }
        }
        best
    }

    /// Append triangle indices whose bounds intersect `aabb`.
    pub fn query_aabb(&self, aabb: Aabb, out: &mut Vec<u32>) {
        if self.triangles.is_empty() || aabb.is_empty() {
            return;
        }
        let mut stack = Vec::with_capacity(64);
        stack.push(0u32);
        while let Some(node_index) = stack.pop() {
            let node = self.nodes[node_index as usize];
            if !node.bounds.intersects(&aabb) {
                continue;
            }
            if node.leaf {
                for i in node.left_or_start..node.left_or_start + node.right_or_count {
                    out.push(self.order[i as usize]);
                }
            } else {
                stack.push(node.left_or_start);
                stack.push(node.right_or_count);
            }
        }
    }

    /// Whether any triangle bound intersects `aabb`. Early-outs, so this is
    /// much cheaper than collecting the candidates (used by the voxeliser to
    /// classify whole 8³ blocks).
    pub fn intersects_aabb(&self, aabb: Aabb) -> bool {
        if self.triangles.is_empty() || aabb.is_empty() {
            return false;
        }
        let mut stack = Vec::with_capacity(32);
        stack.push(0u32);
        while let Some(node_index) = stack.pop() {
            let node = self.nodes[node_index as usize];
            if !node.bounds.intersects(&aabb) {
                continue;
            }
            if node.leaf {
                if node.right_or_count > 0 {
                    return true;
                }
            } else {
                stack.push(node.left_or_start);
                stack.push(node.right_or_count);
            }
        }
        false
    }

    /// Vertices within `radius` of `center`, sorted and deduplicated. This is
    /// the vertex set every brush operates on.
    pub fn vertices_in_sphere(&self, mesh: &Mesh, center: Vec3, radius: f32) -> Vec<u32> {
        let mut triangles = Vec::new();
        self.query_aabb(
            Aabb {
                min: center - Vec3::splat(radius),
                max: center + Vec3::splat(radius),
            },
            &mut triangles,
        );
        let mut seen = vec![false; mesh.positions.len()];
        let mut out = Vec::new();
        let r_sq = radius * radius;
        for tri in triangles {
            let triangle = self.triangles[tri as usize];
            for &v in &triangle {
                if !seen[v as usize] {
                    seen[v as usize] = true;
                    if mesh.positions[v as usize].distance_squared(center) <= r_sq {
                        out.push(v);
                    }
                }
            }
        }
        out.sort_unstable();
        out
    }

    /// Vertices whose projection on the axis plane is within a capsule around
    /// `segment`, expanded by `radius`. Used by chisel-type strokes.
    pub fn vertices_in_capsule(&self, mesh: &Mesh, a: Vec3, b: Vec3, radius: f32) -> Vec<u32> {
        let bounds = Aabb {
            min: a.min(b) - Vec3::splat(radius),
            max: a.max(b) + Vec3::splat(radius),
        };
        let mut triangles = Vec::new();
        self.query_aabb(bounds, &mut triangles);
        let mut seen = vec![false; mesh.positions.len()];
        let mut out = Vec::new();
        let r_sq = radius * radius;
        for tri in triangles {
            for &v in &self.triangles[tri as usize] {
                if seen[v as usize] {
                    continue;
                }
                seen[v as usize] = true;
                let (distance, _) =
                    crate::math::point_segment_distance(mesh.positions[v as usize], a, b);
                if distance <= r_sq.sqrt() {
                    out.push(v);
                }
            }
        }
        out.sort_unstable();
        out
    }

    /// Height of the highest surface point under `(x, y)`; `None` outside.
    pub fn height_under(&self, mesh: &Mesh, x: f32, y: f32) -> Option<f32> {
        if self.triangles.is_empty() {
            return None;
        }
        let top = self.bounds.max.z + 1.0;
        let ray = Ray::new(Vec3::new(x, y, top), Vec3::NEG_Z);
        self.raycast(mesh, &ray, 0.0, f32::INFINITY)
            .map(|hit| hit.position.z)
    }

    /// Signed distance query used by voxelisation and by the physics contacts:
    /// negative inside a closed mesh, positive outside. The sign comes from
    /// the closest triangle's normal, which is reliable within a few cell
    /// widths of the surface.
    pub fn signed_distance(&self, mesh: &Mesh, p: Vec3) -> f32 {
        match self.closest_point(mesh, p) {
            Some(hit) => {
                let distance = hit.distance_sq.sqrt();
                let side = (p - hit.position).dot(hit.normal);
                if side < 0.0 { -distance } else { distance }
            }
            None => f32::INFINITY,
        }
    }
}

fn area_of(bounds: Aabb) -> f32 {
    if bounds.is_empty() {
        return 0.0;
    }
    let s = bounds.size();
    2.0 * (s.x * s.y + s.y * s.z + s.z * s.x)
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
        Mesh::from_faces(positions, &faces)
    }

    #[test]
    fn rays_hit_the_cube_from_outside() {
        let mesh = cube();
        let bvh = Bvh::build(&mesh);
        let ray = Ray::new(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z);
        let hit = bvh.raycast(&mesh, &ray, 0.0, f32::INFINITY).unwrap();
        assert!((hit.t - 4.0).abs() < 1e-4);
        assert!((hit.position.z - 1.0).abs() < 1e-4);
        assert!(hit.normal.dot(Vec3::Z) > 0.9);
        let ray = Ray::new(Vec3::new(5.0, 0.0, 0.0), Vec3::new(-1.0, 0.0, 0.0));
        assert!(bvh.raycast(&mesh, &ray, 0.0, f32::INFINITY).is_some());
    }

    #[test]
    fn closest_point_is_accurate_and_signed_distance_flips() {
        let mesh = cube();
        let bvh = Bvh::build(&mesh);
        let hit = bvh.closest_point(&mesh, Vec3::new(2.0, 0.0, 0.0)).unwrap();
        assert!((hit.position.x - 1.0).abs() < 1e-4);
        assert!((hit.distance_sq - 1.0).abs() < 1e-4);
        assert!(bvh.signed_distance(&mesh, Vec3::ZERO) < 0.0);
        assert!(bvh.signed_distance(&mesh, Vec3::new(3.0, 0.0, 0.0)) > 0.0);
    }

    #[test]
    fn sphere_query_finds_only_nearby_vertices() {
        let mesh = cube();
        let bvh = Bvh::build(&mesh);
        let vertices = bvh.vertices_in_sphere(&mesh, Vec3::new(1.0, 1.0, 1.0), 0.1);
        assert_eq!(vertices, vec![6]);
        let vertices = bvh.vertices_in_sphere(&mesh, Vec3::ZERO, 5.0);
        assert_eq!(vertices.len(), 8);
    }

    #[test]
    fn height_under_returns_top_surface() {
        let mesh = cube();
        let bvh = Bvh::build(&mesh);
        assert!((bvh.height_under(&mesh, 0.2, -0.4).unwrap() - 1.0).abs() < 1e-4);
        assert!(bvh.height_under(&mesh, 9.0, 9.0).is_none());
    }

    /// A wavy grid, large enough that the parallel pre-pass is worth running.
    fn wavy_grid(columns: usize, rows: usize) -> Mesh {
        let mut positions = Vec::with_capacity((columns + 1) * (rows + 1));
        for y in 0..=rows {
            for x in 0..=columns {
                let fx = x as f32 / columns as f32;
                let fy = y as f32 / rows as f32;
                positions.push(Vec3::new(
                    fx * 10.0,
                    fy * 8.0,
                    (fx * 6.0).sin() * 1.5 + (fy * 5.0).cos() * 0.9,
                ));
            }
        }
        let mut faces = Vec::with_capacity(columns * rows);
        let stride = columns + 1;
        for y in 0..rows {
            for x in 0..columns {
                let i = y * stride + x;
                faces.push(vec![
                    i as u32,
                    (i + 1) as u32,
                    (i + stride + 1) as u32,
                    (i + stride) as u32,
                ]);
            }
        }
        Mesh::from_faces(positions, &faces)
    }

    /// The pre-pass runs on the shared pool; the tree must not depend on how
    /// many threads that pool has, or a sculpture would load differently on two
    /// machines.
    #[test]
    fn the_tree_does_not_depend_on_the_thread_count() {
        use std::sync::{Mutex, MutexGuard};

        fn lock() -> MutexGuard<'static, ()> {
            static LOCK: Mutex<()> = Mutex::new(());
            LOCK.lock().unwrap_or_else(|error| error.into_inner())
        }

        let _guard = lock();
        let mesh = wavy_grid(24, 18);
        let snapshot = |bvh: &Bvh| {
            let nodes = bvh
                .nodes
                .iter()
                .map(|node| {
                    (
                        node.bounds.min.to_array(),
                        node.bounds.max.to_array(),
                        node.left_or_start,
                        node.right_or_count,
                        node.leaf,
                    )
                })
                .collect::<Vec<_>>();
            (nodes, bvh.order.clone(), bvh.bounds.min.to_array())
        };
        let previous = vf_core::compute::current_threads();
        let mut reference = None;
        for threads in [1usize, 3, 0] {
            vf_core::compute::configure_threads(threads).expect("pool de calcul");
            let actual = snapshot(&Bvh::build(&mesh));
            match &reference {
                Some(expected) => assert_eq!(&actual, expected, "{threads} threads"),
                None => reference = Some(actual),
            }
        }
        let _ = vf_core::compute::configure_threads(previous);
    }

    #[test]
    fn refit_follows_vertex_motion() {
        let mut mesh = cube();
        let mut bvh = Bvh::build(&mesh);
        for p in &mut mesh.positions {
            p.z += 2.0;
        }
        bvh.refit(&mesh);
        let ray = Ray::new(Vec3::new(0.0, 0.0, 8.0), Vec3::NEG_Z);
        let hit = bvh.raycast(&mesh, &ray, 0.0, f32::INFINITY).unwrap();
        assert!((hit.position.z - 3.0).abs() < 1e-4);
    }
}
