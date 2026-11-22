//! Voxel grids, narrow-band signed distance fields, surface-nets
//! polygonisation and constructive solid geometry.
//!
//! # Why voxels
//!
//! A triangle mesh can be sculpted and remeshed, but a boolean between two
//! detailed meshes, a blob fusion or a guaranteed watertight result is much
//! easier on a distance field. This module converts a closed mesh into a
//! narrow-band SDF, combines fields (union, difference, intersection, smooth
//! blends), and extracts a quad mesh with *surface nets*: one vertex per cell
//! that the surface crosses, one quad per grid edge that changes sign. The
//! result is watertight, quad-dominant and free of the T-junctions that plague
//! adaptive grids.
//!
//! # Sign convention
//!
//! Negative inside, positive outside, values clamped to `±band`. Only voxels
//! in a narrow band around the surface hold a real distance; the rest hold
//! `+band` (outside) or `-band` (inside). Voxels are marked *resolved* when the
//! band pass wrote them, so the interior/outside flood fill cannot leak through
//! a coarse band.
//!
//! # Conventions
//!
//! Value `(i, j, k)` is the field at the grid point
//! `origin + (i, j, k) * cell`. A cell `(i, j, k)` spans the eight grid points
//! from `(i, j, k)` to `(i + 1, j + 1, k + 1)`.

use std::collections::VecDeque;

use glam::Vec3;
use rayon::prelude::*;

use crate::bvh::Bvh;
use crate::math::{Aabb, smoothstep};
use crate::mesh::Mesh;

/// Boolean operations between two fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BooleanOp {
    Union,
    Difference,
    Intersection,
}

impl BooleanOp {
    pub const ALL: [BooleanOp; 3] = [
        BooleanOp::Union,
        BooleanOp::Difference,
        BooleanOp::Intersection,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BooleanOp::Union => "Union",
            BooleanOp::Difference => "Soustraction",
            BooleanOp::Intersection => "Intersection",
        }
    }
}

/// Parameters for mesh to SDF conversion.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VoxelParams {
    /// Requested voxel size, metres.
    pub cell_m: f32,
    /// Half-thickness of the measured band, in cells.
    pub band_cells: f32,
    /// Empty margin around the mesh, in cells.
    pub padding_cells: usize,
    /// Hard cap on the number of voxels. The cell size is increased until the
    /// grid fits, so a tiny voxel size on a huge landscape cannot exhaust RAM.
    pub max_voxels: usize,
}

impl Default for VoxelParams {
    fn default() -> Self {
        Self {
            cell_m: 0.5,
            band_cells: 2.5,
            padding_cells: 2,
            max_voxels: 64_000_000,
        }
    }
}

impl VoxelParams {
    pub fn with_cell(cell_m: f32) -> Self {
        Self {
            cell_m,
            ..Self::default()
        }
    }

    /// Cell size that respects `max_voxels` for the given bounds.
    pub fn fitted_cell(&self, bounds: &Aabb) -> f32 {
        let requested = self.cell_m.max(1e-4);
        let budget = self.max_voxels.max(4096) as f64;
        let size = bounds.size() + Vec3::splat(requested * 2.0);
        let volume = (size.x as f64 * size.y as f64 * size.z as f64).max(1e-9);
        // One voxel per `cell³`; leave 10 % of headroom for padding.
        let required = (volume / (budget * 0.9)).cbrt() as f32;
        let mut cell = requested.max(required).max(1e-4);
        for _ in 0..32 {
            let dims = (size / cell).ceil() + Vec3::splat(1.0);
            let count = (dims.x as u64) * (dims.y as u64) * (dims.z as u64);
            if count <= budget as u64 {
                break;
            }
            cell *= 1.1;
        }
        cell
    }
}

/// A regular grid of signed distances (negative inside) plus a per-voxel
/// resolution flag used by the interior flood fill.
#[derive(Debug, Clone)]
pub struct Grid {
    pub origin: Vec3,
    pub cell: f32,
    pub dims: [usize; 3],
    pub values: Vec<f32>,
    /// 0 = unresolved, 1 = measured band, 2 = outside (visited by the flood).
    pub state: Vec<u8>,
    pub band: f32,
}

impl Grid {
    pub fn new(bounds: &Aabb, cell: f32, padding_cells: usize) -> Self {
        let cell = cell.max(1e-4);
        let padding = padding_cells as f32 * cell;
        let min = bounds.min - Vec3::splat(padding);
        let max = bounds.max + Vec3::splat(padding);
        let dims = [
            (((max.x - min.x) / cell).ceil() as usize + 1).max(2),
            (((max.y - min.y) / cell).ceil() as usize + 1).max(2),
            (((max.z - min.z) / cell).ceil() as usize + 1).max(2),
        ];
        let count = dims[0] * dims[1] * dims[2];
        Self {
            origin: min,
            cell,
            dims,
            values: vec![cell * 2.5; count],
            state: vec![0; count],
            band: cell * 2.5,
        }
    }

    pub fn with_dims(origin: Vec3, cell: f32, dims: [usize; 3], band: f32) -> Self {
        let count = dims[0] * dims[1] * dims[2];
        Self {
            origin,
            cell,
            dims,
            values: vec![band; count],
            state: vec![0; count],
            band,
        }
    }

    #[inline]
    pub fn index(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.dims[1] + y) * self.dims[0] + x
    }

    #[inline]
    pub fn coordinates(&self, index: usize) -> (usize, usize, usize) {
        let yz = self.dims[0] * self.dims[1];
        let z = index / yz;
        let rest = index - z * yz;
        let y = rest / self.dims[0];
        let x = rest - y * self.dims[0];
        (x, y, z)
    }

    #[inline]
    pub fn point(&self, x: usize, y: usize, z: usize) -> Vec3 {
        self.origin + Vec3::new(x as f32, y as f32, z as f32) * self.cell
    }

    #[inline]
    pub fn value(&self, x: usize, y: usize, z: usize) -> f32 {
        self.values[self.index(x, y, z)]
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, z: usize, value: f32) {
        let index = self.index(x, y, z);
        self.values[index] = value;
        self.state[index] = 1;
    }

    pub fn voxel_count(&self) -> usize {
        self.values.len()
    }

    pub fn bounds(&self) -> Aabb {
        Aabb::new(
            self.origin,
            self.origin
                + Vec3::new(
                    (self.dims[0] - 1) as f32,
                    (self.dims[1] - 1) as f32,
                    (self.dims[2] - 1) as f32,
                ) * self.cell,
        )
    }

    /// Trilinear sample at a world position.
    pub fn sample(&self, p: Vec3) -> f32 {
        let g = (p - self.origin) / self.cell;
        let fx = g.x.clamp(0.0, (self.dims[0] - 1) as f32);
        let fy = g.y.clamp(0.0, (self.dims[1] - 1) as f32);
        let fz = g.z.clamp(0.0, (self.dims[2] - 1) as f32);
        let x0 = (fx.floor() as usize).min(self.dims[0] - 1);
        let y0 = (fy.floor() as usize).min(self.dims[1] - 1);
        let z0 = (fz.floor() as usize).min(self.dims[2] - 1);
        let x1 = (x0 + 1).min(self.dims[0] - 1);
        let y1 = (y0 + 1).min(self.dims[1] - 1);
        let z1 = (z0 + 1).min(self.dims[2] - 1);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let tz = fz - z0 as f32;
        let v000 = self.value(x0, y0, z0);
        let v100 = self.value(x1, y0, z0);
        let v010 = self.value(x0, y1, z0);
        let v110 = self.value(x1, y1, z0);
        let v001 = self.value(x0, y0, z1);
        let v101 = self.value(x1, y0, z1);
        let v011 = self.value(x0, y1, z1);
        let v111 = self.value(x1, y1, z1);
        let v00 = v000 + (v100 - v000) * tx;
        let v10 = v010 + (v110 - v010) * tx;
        let v01 = v001 + (v101 - v001) * tx;
        let v11 = v011 + (v111 - v011) * tx;
        let v0 = v00 + (v10 - v00) * ty;
        let v1 = v01 + (v11 - v01) * ty;
        v0 + (v1 - v0) * tz
    }

    /// Central-difference gradient at a world position.
    pub fn gradient(&self, p: Vec3) -> Vec3 {
        let h = self.cell;
        Vec3::new(
            self.sample(p + Vec3::X * h) - self.sample(p - Vec3::X * h),
            self.sample(p + Vec3::Y * h) - self.sample(p - Vec3::Y * h),
            self.sample(p + Vec3::Z * h) - self.sample(p - Vec3::Z * h),
        ) / (2.0 * h)
    }

    /// Move a point onto the isosurface with a few Newton steps. The step is
    /// clamped to half a cell so a bad gradient cannot throw a vertex across
    /// the grid.
    pub fn project_to_surface(&self, p: Vec3, iterations: u32) -> Vec3 {
        let mut point = p;
        for _ in 0..iterations {
            let value = self.sample(point);
            let gradient = self.gradient(point);
            let denom = gradient.length_squared();
            if denom < 1e-8 {
                break;
            }
            let step = gradient * (value / denom);
            let limit = self.cell * 0.5;
            let length = step.length();
            let step = if length > limit {
                step * (limit / length)
            } else {
                step
            };
            point -= step;
            if step.length() < self.cell * 1e-3 {
                break;
            }
        }
        point
    }

    /// Replace unresolved outside voxels with the measured band and negative
    /// interior with `-band`. The flood only walks unresolved voxels, so the
    /// measured shell blocks it.
    fn fill_interior(&mut self) {
        let [nx, ny, nz] = self.dims;
        let mut queue = VecDeque::new();
        fn push_seed(grid: &mut Grid, x: usize, y: usize, z: usize, queue: &mut VecDeque<usize>) {
            let index = grid.index(x, y, z);
            if grid.state[index] == 0 {
                grid.state[index] = 2;
                queue.push_back(index);
            }
        }
        for z in 0..nz {
            for y in 0..ny {
                push_seed(self, 0, y, z, &mut queue);
                push_seed(self, nx - 1, y, z, &mut queue);
            }
        }
        for z in 0..nz {
            for x in 0..nx {
                push_seed(self, x, 0, z, &mut queue);
                push_seed(self, x, ny - 1, z, &mut queue);
            }
        }
        for y in 0..ny {
            for x in 0..nx {
                push_seed(self, x, y, 0, &mut queue);
                push_seed(self, x, y, nz - 1, &mut queue);
            }
        }
        let yz = nx * ny;
        while let Some(index) = queue.pop_front() {
            let z = index / yz;
            let rest = index - z * yz;
            let y = rest / nx;
            let x = rest - y * nx;
            let neighbours = [
                (x.wrapping_sub(1), y, z),
                (x + 1, y, z),
                (x, y.wrapping_sub(1), z),
                (x, y + 1, z),
                (x, y, z.wrapping_sub(1)),
                (x, y, z + 1),
            ];
            for (nx_, ny_, nz_) in neighbours {
                if nx_ < nx && ny_ < ny && nz_ < nz {
                    let n_index = self.index(nx_, ny_, nz_);
                    if self.state[n_index] == 0 {
                        self.state[n_index] = 2;
                        queue.push_back(n_index);
                    }
                }
            }
        }
        for (index, value) in self.values.iter_mut().enumerate() {
            match self.state[index] {
                0 => *value = -self.band,
                2 => *value = self.band,
                _ => *value = value.clamp(-self.band, self.band),
            }
        }
    }
}

/// Convert a closed triangle mesh into a narrow-band signed distance grid.
///
/// The mesh should be watertight (`terrain::prepare_solid` closes a terrain
/// sheet). An open mesh still produces a band but its sign is only meaningful
/// near the surface.
pub fn voxelize(mesh: &Mesh, bvh: &Bvh, params: VoxelParams) -> Grid {
    let bounds = mesh.bounds();
    let cell = params.fitted_cell(&bounds);
    let padding = params.padding_cells.max(1);
    let mut grid = Grid::new(&bounds, cell, padding);
    let band = cell * params.band_cells.max(1.5);
    grid.band = band;

    // Active 8³ blocks: those whose expanded bounds touch a triangle.
    const BLOCK: usize = 8;
    let [nx, ny, nz] = grid.dims;
    let bx = nx.div_ceil(BLOCK);
    let by = ny.div_ceil(BLOCK);
    let bz = nz.div_ceil(BLOCK);
    let mut active = vec![0u8; bx * by * bz];
    vf_core::compute::install(|| {
        active.par_iter_mut().enumerate().for_each(|(index, flag)| {
            let x = index % bx;
            let y = (index / bx) % by;
            let z = index / (bx * by);
            let min = grid.point(x * BLOCK, y * BLOCK, z * BLOCK) - Vec3::splat(band + cell);
            let max = grid.point(
                ((x + 1) * BLOCK).min(nx - 1),
                ((y + 1) * BLOCK).min(ny - 1),
                ((z + 1) * BLOCK).min(nz - 1),
            ) + Vec3::splat(band + cell);
            if bvh.intersects_aabb(Aabb::new(min, max)) {
                *flag = 1;
            }
        });
    });

    // Fill the band, one row at a time (rows are independent).
    let row_width = nx;
    let origin = grid.origin;
    let voxel_cell = grid.cell;
    let Grid {
        values: grid_values,
        state: grid_state,
        ..
    } = &mut grid;
    vf_core::compute::install(|| {
        grid_values
            .par_chunks_mut(row_width)
            .zip(grid_state.par_chunks_mut(row_width))
            .enumerate()
            .for_each(|(row, (values, states))| {
                let y = row % ny;
                let z = row / ny;
                if z >= nz || y >= ny {
                    return;
                }
                for x in 0..nx {
                    let block = (x / BLOCK) + bx * (y / BLOCK) + bx * by * (z / BLOCK);
                    if active[block] == 0 {
                        continue;
                    }
                    let p = origin + Vec3::new(x as f32, y as f32, z as f32) * voxel_cell;
                    let Some(hit) = bvh.closest_point(mesh, p) else {
                        continue;
                    };
                    let distance = hit.distance_sq.sqrt();
                    let side = (p - hit.position).dot(hit.normal);
                    let signed = if side < 0.0 { -distance } else { distance };
                    values[x] = signed.clamp(-band, band);
                    states[x] = 1;
                }
            });
    });

    grid.fill_interior();
    grid
}

/// Build a grid from a signed field evaluated on every voxel (negative
/// inside). Used by metaballs and by synthetic shapes.
pub fn from_field(
    bounds: &Aabb,
    cell: f32,
    max_voxels: usize,
    field: impl Fn(Vec3) -> f32 + Sync,
) -> Grid {
    let params = VoxelParams {
        cell_m: cell,
        max_voxels,
        ..VoxelParams::default()
    };
    let cell = params.fitted_cell(bounds);
    let mut grid = Grid::new(bounds, cell, 1);
    let [nx, ny, nz] = grid.dims;
    let origin = grid.origin;
    let band = grid.band;
    let Grid {
        values: grid_values,
        ..
    } = &mut grid;
    vf_core::compute::install(|| {
        grid_values
            .par_chunks_mut(nx)
            .enumerate()
            .for_each(|(row, values)| {
                let y = row % ny;
                let z = row / ny;
                if y >= ny || z >= nz {
                    return;
                }
                for (x, value) in values.iter_mut().enumerate() {
                    let p = origin + Vec3::new(x as f32, y as f32, z as f32) * cell;
                    *value = field(p).clamp(-band, band);
                }
            });
    });
    grid.state.fill(1);
    grid
}

/// Smooth minimum used for blob unions.
pub fn smin(a: f32, b: f32, k: f32) -> f32 {
    if k <= 1e-6 {
        return a.min(b);
    }
    let h = ((k - (a - b).abs()) / k).clamp(0.0, 1.0);
    a.min(b) - h * h * k * 0.25
}

/// Smooth maximum used for blob differences.
pub fn smax(a: f32, b: f32, k: f32) -> f32 {
    -smin(-a, -b, k)
}

/// Combine two grids that share origin, cell and dimensions.
pub fn boolean(a: &Grid, b: &Grid, op: BooleanOp, smooth_k: f32) -> Grid {
    assert_eq!(a.dims, b.dims, "boolean grids must share dimensions");
    let mut out = a.clone();
    for (index, value) in out.values.iter_mut().enumerate() {
        let va = a.values[index];
        let vb = b.values[index];
        *value = match op {
            BooleanOp::Union => {
                if smooth_k > 0.0 {
                    smin(va, vb, smooth_k)
                } else {
                    va.min(vb)
                }
            }
            BooleanOp::Difference => {
                if smooth_k > 0.0 {
                    smax(va, -vb, smooth_k)
                } else {
                    va.max(-vb)
                }
            }
            BooleanOp::Intersection => {
                if smooth_k > 0.0 {
                    smax(va, vb, smooth_k)
                } else {
                    va.max(vb)
                }
            }
        };
    }
    out.state.fill(1);
    out.band = a.band;
    out
}

/// Extract a quad mesh from the zero level set with surface nets.
///
/// One vertex per crossing cell, one quad per sign-changing grid edge. Vertices
/// are projected onto the isosurface with a few Newton steps, which removes the
/// "staircase" of naive voxel meshing while keeping the topological guarantees.
pub fn surface_nets(grid: &Grid, newton_iterations: u32) -> Mesh {
    let [nx, ny, nz] = grid.dims;
    if nx < 2 || ny < 2 || nz < 2 {
        return Mesh::new();
    }
    let cells = [(nx - 1), (ny - 1), (nz - 1)];
    let cell_index = |x: usize, y: usize, z: usize| (z * cells[1] + y) * cells[0] + x;
    let mut cell_vertex = vec![u32::MAX; cells[0] * cells[1] * cells[2]];
    let mut positions: Vec<Vec3> = Vec::new();
    let mut normals: Vec<Vec3> = Vec::new();

    // One vertex per crossing cell.
    for z in 0..cells[2] {
        for y in 0..cells[1] {
            for x in 0..cells[0] {
                let corners = [
                    grid.value(x, y, z),
                    grid.value(x + 1, y, z),
                    grid.value(x, y + 1, z),
                    grid.value(x + 1, y + 1, z),
                    grid.value(x, y, z + 1),
                    grid.value(x + 1, y, z + 1),
                    grid.value(x, y + 1, z + 1),
                    grid.value(x + 1, y + 1, z + 1),
                ];
                let mut inside = 0;
                for v in corners {
                    if v < 0.0 {
                        inside += 1;
                    }
                }
                if inside == 0 || inside == 8 {
                    continue;
                }
                // Average the crossing points of the twelve cell edges.
                let offsets = [
                    (0usize, 1usize),
                    (2, 3),
                    (4, 5),
                    (6, 7),
                    (0, 2),
                    (1, 3),
                    (4, 6),
                    (5, 7),
                    (0, 4),
                    (1, 5),
                    (2, 6),
                    (3, 7),
                ];
                let corner_offset = [
                    Vec3::new(0.0, 0.0, 0.0),
                    Vec3::new(1.0, 0.0, 0.0),
                    Vec3::new(0.0, 1.0, 0.0),
                    Vec3::new(1.0, 1.0, 0.0),
                    Vec3::new(0.0, 0.0, 1.0),
                    Vec3::new(1.0, 0.0, 1.0),
                    Vec3::new(0.0, 1.0, 1.0),
                    Vec3::new(1.0, 1.0, 1.0),
                ];
                let mut sum = Vec3::ZERO;
                let mut count = 0.0;
                for (a, b) in offsets {
                    let va = corners[a];
                    let vb = corners[b];
                    if (va < 0.0) != (vb < 0.0) {
                        let denom = va - vb;
                        let t = if denom.abs() < 1e-12 { 0.5 } else { va / denom };
                        let local = corner_offset[a] + (corner_offset[b] - corner_offset[a]) * t;
                        sum += local;
                        count += 1.0;
                    }
                }
                if count < 3.0 {
                    continue;
                }
                let local = sum / count;
                let base = grid.point(x, y, z);
                let mut point = base + local * grid.cell;
                if newton_iterations > 0 {
                    point = grid.project_to_surface(point, newton_iterations);
                    let lo = base;
                    let hi = base + Vec3::splat(grid.cell);
                    point = point.clamp(lo, hi);
                }
                let gradient = grid.gradient(point);
                let normal = gradient.normalize_or_zero();
                let id = positions.len() as u32;
                positions.push(point);
                normals.push(if normal.length_squared() < 0.5 {
                    Vec3::Z
                } else {
                    normal
                });
                cell_vertex[cell_index(x, y, z)] = id;
            }
        }
    }

    let mut faces: Vec<[u32; 4]> = Vec::new();
    let quad = |ids: [u32; 4], outward: Vec3, faces: &mut Vec<[u32; 4]>| {
        if ids.contains(&u32::MAX) {
            return;
        }
        let a = positions[ids[0] as usize];
        let b = positions[ids[1] as usize];
        let c = positions[ids[2] as usize];
        let normal = (b - a).cross(c - a);
        if normal.dot(outward) < 0.0 {
            faces.push([ids[0], ids[3], ids[2], ids[1]]);
        } else {
            faces.push(ids);
        }
    };

    // X edges: the four cells around the edge live in the (y, z) plane.
    for z in 1..cells[2] {
        for y in 1..cells[1] {
            for x in 0..cells[0] {
                let v0 = grid.value(x, y, z);
                let v1 = grid.value(x + 1, y, z);
                if (v0 < 0.0) == (v1 < 0.0) {
                    continue;
                }
                let inward = if v0 < 0.0 { Vec3::X } else { Vec3::NEG_X };
                let ids = [
                    cell_vertex[cell_index(x, y - 1, z - 1)],
                    cell_vertex[cell_index(x, y - 1, z)],
                    cell_vertex[cell_index(x, y, z)],
                    cell_vertex[cell_index(x, y, z - 1)],
                ];
                quad(ids, inward, &mut faces);
            }
        }
    }
    // Y edges.
    for z in 1..cells[2] {
        for y in 0..cells[1] {
            for x in 1..cells[0] {
                let v0 = grid.value(x, y, z);
                let v1 = grid.value(x, y + 1, z);
                if (v0 < 0.0) == (v1 < 0.0) {
                    continue;
                }
                let outward = if v0 < 0.0 { Vec3::Y } else { Vec3::NEG_Y };
                let ids = [
                    cell_vertex[cell_index(x - 1, y, z - 1)],
                    cell_vertex[cell_index(x, y, z - 1)],
                    cell_vertex[cell_index(x, y, z)],
                    cell_vertex[cell_index(x - 1, y, z)],
                ];
                quad(ids, outward, &mut faces);
            }
        }
    }
    // Z edges.
    for z in 0..cells[2] {
        for y in 1..cells[1] {
            for x in 1..cells[0] {
                let v0 = grid.value(x, y, z);
                let v1 = grid.value(x, y, z + 1);
                if (v0 < 0.0) == (v1 < 0.0) {
                    continue;
                }
                let outward = if v0 < 0.0 { Vec3::Z } else { Vec3::NEG_Z };
                let ids = [
                    cell_vertex[cell_index(x - 1, y - 1, z)],
                    cell_vertex[cell_index(x, y - 1, z)],
                    cell_vertex[cell_index(x, y, z)],
                    cell_vertex[cell_index(x - 1, y, z)],
                ];
                quad(ids, outward, &mut faces);
            }
        }
    }

    let mut mesh = Mesh::from_quads(positions, faces);
    mesh.normals = normals;
    // Clean the dual mesh: surface nets can pinch a cell where two sheets of a
    // CSG seam meet. Removing the zero-area quads and capping the resulting
    // micro-holes gives a closed surface again; a handful of non-manifold
    // edges can survive a perfectly sharp boolean and are reported by the
    // analysis panel rather than hidden.
    mesh.weld(grid.cell * 1e-5);
    mesh.collapse_repeated_corners();
    mesh.remove_degenerate_faces((grid.cell * grid.cell * 1e-9).max(1e-14));
    // A pinched dual vertex can leave a short open chain after the first cap;
    // two extra passes close the remainder without inventing large faces.
    for _ in 0..3 {
        if mesh.boundary_loops().is_empty() {
            break;
        }
        mesh.cap_boundary_loops();
    }
    // `boundary_loops` cannot chain a hole whose boundary touches itself at a
    // non-manifold vertex. Close whatever it left with a per-component fan.
    for _ in 0..2 {
        if mesh.cap_boundary_edges() == 0 {
            break;
        }
    }
    mesh.box_project_uvs(1.0);
    mesh.recompute_normals_if_needed();
    mesh
}

/// Taubin smoothing (λ|μ) of the vertex positions: removes voxel-scale
/// ripples without the volume loss of a plain Laplacian pass.
///
/// The neighbour table is a sorted CSR, so every vertex sums its neighbours in
/// the same order whatever the thread count; the passes run on the compute
/// backend chosen in the preferences.
pub fn taubin_smooth(mesh: &mut Mesh, iterations: u32, lambda: f32, mu: f32) {
    if mesh.is_empty() || iterations == 0 {
        return;
    }
    let graph = mesh.face_graph();
    let neighbours = vf_compute::Neighbours::build(mesh.positions.len(), &graph);
    let smoothed = vf_compute::smooth(&mesh.positions, &neighbours, iterations, lambda, mu).value;
    if smoothed.len() == mesh.positions.len() {
        mesh.positions = smoothed;
    }
    mesh.recompute_normals();
}

/// Convenience: voxelise a mesh and return a quad remesh.
pub fn voxel_remesh(mesh: &Mesh, bvh: &Bvh, params: VoxelParams, smooth: bool) -> Mesh {
    let grid = voxelize(mesh, bvh, params);
    let mut out = surface_nets(&grid, 2);
    if smooth {
        taubin_smooth(&mut out, 2, 0.5, -0.53);
    }
    out
}

/// Local SDF stamping: carve or add a field around a point without going
/// through a full mesh. Returns the grid and the affected bounds so a caller
/// can replace only that region.
pub fn stamp_sphere(grid: &mut Grid, center: Vec3, radius: f32, strength: f32) {
    let r = radius.max(grid.cell);
    let min = ((center - Vec3::splat(r) - grid.origin) / grid.cell).floor();
    let max = ((center + Vec3::splat(r) - grid.origin) / grid.cell).ceil();
    let x0 = (min.x.max(0.0) as usize).min(grid.dims[0] - 1);
    let y0 = (min.y.max(0.0) as usize).min(grid.dims[1] - 1);
    let z0 = (min.z.max(0.0) as usize).min(grid.dims[2] - 1);
    let x1 = (max.x.max(0.0) as usize).min(grid.dims[0] - 1);
    let y1 = (max.y.max(0.0) as usize).min(grid.dims[1] - 1);
    let z1 = (max.z.max(0.0) as usize).min(grid.dims[2] - 1);
    for z in z0..=z1 {
        for y in y0..=y1 {
            for x in x0..=x1 {
                let p = grid.point(x, y, z);
                let distance = (p - center).length() - r;
                let index = grid.index(x, y, z);
                grid.values[index] =
                    smax(grid.values[index], -distance * strength, grid.cell * 0.5);
                grid.state[index] = 1;
            }
        }
    }
}

/// Smoothstep helper exported for the cliff shaper.
pub fn band_fade(value: f32, inner: f32, outer: f32) -> f32 {
    smoothstep(inner, outer, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::Mesh;

    fn cube(size: f32) -> Mesh {
        let h = size * 0.5;
        let positions = vec![
            Vec3::new(-h, -h, -h),
            Vec3::new(h, -h, -h),
            Vec3::new(h, h, -h),
            Vec3::new(-h, h, -h),
            Vec3::new(-h, -h, h),
            Vec3::new(h, -h, h),
            Vec3::new(h, h, h),
            Vec3::new(-h, h, h),
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

    fn sphere(radius: f32, center: Vec3) -> Mesh {
        let grid = from_field(
            &Aabb::new(center - Vec3::splat(radius), center + Vec3::splat(radius)),
            radius / 12.0,
            4_000_000,
            |p| (p - center).length() - radius,
        );
        surface_nets(&grid, 2)
    }

    #[test]
    fn narrow_band_has_correct_signs_and_is_closed() {
        let mesh = cube(2.0);
        let bvh = Bvh::build(&mesh);
        let grid = voxelize(&mesh, &bvh, VoxelParams::with_cell(0.1));
        assert!(grid.sample(Vec3::ZERO) < 0.0);
        assert!(grid.sample(Vec3::new(2.0, 0.0, 0.0)) > 0.0);
        assert!(grid.sample(Vec3::new(0.99, 0.0, 0.0)).abs() < 0.15);
        assert!(grid.values.iter().all(|v| v.abs() <= grid.band + 1e-5));
    }

    #[test]
    fn surface_nets_reconstructs_a_watertight_cube_with_quads() {
        let mesh = cube(2.0);
        let bvh = Bvh::build(&mesh);
        let grid = voxelize(&mesh, &bvh, VoxelParams::with_cell(0.125));
        let out = surface_nets(&grid, 2);
        let stats = out.stats();
        assert!(stats.quads > 0);
        assert!(stats.triangles > 100);
        assert_eq!(stats.boundary_edges, 0, "{stats:?}");
        assert_eq!(stats.non_manifold_edges, 0, "{stats:?}");
        assert_eq!(stats.components, 1);
        assert!((out.volume() - 8.0).abs() < 1.5, "volume {}", out.volume());
        assert!(out.positions.iter().all(|p| p.is_finite()));
    }

    #[test]
    fn surface_nets_sphere_is_round() {
        let mesh = sphere(1.0, Vec3::ZERO);
        let stats = mesh.stats();
        assert!(stats.watertight, "{stats:?}");
        assert!(stats.quads > stats.tris);
        let volume = mesh.volume();
        let expected = 4.0 / 3.0 * std::f64::consts::PI;
        assert!((volume - expected).abs() < 0.35, "{volume} vs {expected}");
    }

    #[test]
    fn boolean_difference_removes_material() {
        let a = cube(2.0);
        let b = sphere(1.15, Vec3::new(1.0, 1.0, 1.0));
        let params = VoxelParams {
            cell_m: 0.12,
            max_voxels: 8_000_000,
            ..VoxelParams::default()
        };
        let ga = voxelize(&a, &Bvh::build(&a), params);
        let gb = voxelize(&b, &Bvh::build(&b), params);
        // Shared grid: voxelise both on the union bounds instead.
        let bounds = a.bounds().union(b.bounds());
        let cell = params.fitted_cell(&bounds);
        let mut shared_a = Grid::new(&bounds, cell, 2);
        shared_a.band = cell * 2.5;
        let mut shared_b = shared_a.clone();
        copy_into(&ga, &mut shared_a);
        copy_into(&gb, &mut shared_b);
        let diff = boolean(&shared_a, &shared_b, BooleanOp::Difference, 0.0);
        let out = surface_nets(&diff, 1);
        let stats = out.stats();
        assert!(stats.watertight, "{stats:?}");
        assert!(out.volume() < a.volume());
        assert!(out.volume() > a.volume() * 0.5);
    }

    fn copy_into(source: &Grid, target: &mut Grid) {
        for z in 0..target.dims[2] {
            for y in 0..target.dims[1] {
                for x in 0..target.dims[0] {
                    let p = target.point(x, y, z);
                    let index = target.index(x, y, z);
                    target.values[index] = source.sample(p);
                    target.state[index] = 1;
                }
            }
        }
    }

    #[test]
    fn smooth_min_blends_below_the_minimum() {
        assert!((smin(0.5, 0.7, 0.3) - 0.491_667).abs() < 1e-4);
        assert!((smin(1.0, 2.0, 0.0) - 1.0).abs() < 1e-6);
        assert!(smin(0.0, 0.0, 1.0) < 0.0);
    }

    #[test]
    fn fitted_cell_respects_the_budget() {
        let bounds = Aabb::new(Vec3::ZERO, Vec3::splat(100.0));
        let params = VoxelParams {
            cell_m: 0.01,
            max_voxels: 1_000_000,
            ..VoxelParams::default()
        };
        let cell = params.fitted_cell(&bounds);
        let dims = 100.0 / cell;
        assert!((dims * dims * dims) < 1_500_000.0, "cell {cell}");
    }
}
