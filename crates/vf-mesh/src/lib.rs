//! Mesh construction from the detailed surface.
//!
//! The renderer used to upload the heightfield grid at full resolution. This
//! crate builds a mesh that can be denser than the stored grid (subdivision)
//! while evaluating the continuous detail field at every new vertex, or
//! cheaper than it when a budget is exceeded. The mesh is the single source
//! for the CPU renderer and the glTF/Blender exporter, so what you see is what
//! you export.
//!
//! The builder is deterministic and parallel across rows.

use rayon::prelude::*;
use vf_core::HeightField;
use vf_detail::DetailSampler;

/// World-space view used to derive a conservative pixel footprint for
/// band-limiting. `None` disables distance-based filtering (export mode).
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub camera: [f64; 3],
    pub fov_y_rad: f32,
    pub pixel_height: u32,
}

impl View {
    pub fn footprint(&self, x: f64, y: f64, z: f32, sample_spacing: f32) -> f32 {
        let distance = ((x - self.camera[0]).powi(2)
            + (y - self.camera[1]).powi(2)
            + (f64::from(z) - self.camera[2]).powi(2))
        .sqrt() as f32;
        let pixel = distance * 2.0 * (self.fov_y_rad * 0.5).tan() / self.pixel_height.max(1) as f32;
        pixel.max(sample_spacing * 0.75)
    }
}

/// A triangle mesh in absolute world coordinates (metres, Z-up).
#[derive(Debug, Clone, Default)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<[u32; 3]>,
    /// Optional quad/n-gon topology from the shape-adaptive builder. `indices`
    /// always carries a triangulation for the renderer; this list is what the
    /// Blender OBJ export writes when the quad option is selected.
    pub polygons: Vec<Vec<u32>>,
    /// Grid dimensions used by the builder (rows/columns of vertices).
    pub nx: usize,
    pub ny: usize,
    /// Effective horizontal sample spacing, metres.
    pub spacing_m: f32,
    /// Effective vertices per heightfield cell (>= 1 means subdivision).
    pub density: f32,
}

impl MeshData {
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len()
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Recompute smooth normals from the triangles. Used by export paths that
    /// decimate or displace the mesh after construction.
    pub fn recompute_normals(&mut self) {
        let mut normals = vec![[0.0f32; 3]; self.positions.len()];
        for [a, b, c] in &self.indices {
            let pa = self.positions[*a as usize];
            let pb = self.positions[*b as usize];
            let pc = self.positions[*c as usize];
            let e1 = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
            let e2 = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
            let n = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            for index in [*a as usize, *b as usize, *c as usize] {
                normals[index][0] += n[0];
                normals[index][1] += n[1];
                normals[index][2] += n[2];
            }
        }
        for (normal, out) in normals.iter().zip(self.normals.iter_mut()) {
            let len = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2])
                .sqrt()
                .max(1e-12);
            *out = [normal[0] / len, normal[1] / len, normal[2] / len];
        }
    }
}

/// Surface evaluated by the adaptive builder. `DetailSampler` is the base
/// implementation; materials add a vertical displacement on top of it so the
/// adaptive pass also resolves bump/displacement.
pub trait SurfaceSampler {
    fn height(&self, x: f64, y: f64, footprint_m: f32) -> f32;
    fn normal(&self, x: f64, y: f64, footprint_m: f32) -> [f32; 3];
    fn height_field(&self) -> &HeightField;
}

impl SurfaceSampler for DetailSampler<'_> {
    fn height(&self, x: f64, y: f64, footprint_m: f32) -> f32 {
        DetailSampler::height(self, x, y, footprint_m)
    }

    fn normal(&self, x: f64, y: f64, footprint_m: f32) -> [f32; 3] {
        DetailSampler::normal(self, x, y, footprint_m)
    }

    fn height_field(&self) -> &HeightField {
        DetailSampler::height_field(self)
    }
}

/// Build a regular mesh over the whole heightfield.
///
/// * `subdivision` is the number of vertices per cell (1 = base grid, 2 =
///   twice as dense in each direction).
/// * `max_vertices` bounds the allocation; the builder coarsens the grid when
///   the requested subdivision would exceed it. The base grid is used as a
///   last resort even if it still exceeds the budget, so a result is always
///   returned.
pub fn build(
    sampler: &DetailSampler<'_>,
    subdivision: u32,
    max_vertices: usize,
    view: Option<View>,
) -> MeshData {
    let hf = sampler.height_field();
    let (w, h) = (hf.width(), hf.height());
    let cells_x = (w - 1) as f64;
    let cells_y = (h - 1) as f64;
    let density = subdivision.clamp(1, 8) as f64;

    // Start at the requested density and coarsen until the vertex budget is
    // met. `step` is expressed in heightfield cells per mesh vertex.
    let mut step = 1.0 / density;
    let mut nx;
    let mut ny;
    let max_vertices = max_vertices.max(16);
    loop {
        nx = (cells_x / step).ceil() as usize + 1;
        ny = (cells_y / step).ceil() as usize + 1;
        if nx * ny <= max_vertices || step >= 32.0 {
            break;
        }
        step *= 1.25;
    }
    nx = nx.max(2);
    ny = ny.max(2);

    let (origin_x, origin_y) = hf.origin();
    let (spacing_x, spacing_y) = hf.spacing();
    let x_step = cells_x / (nx - 1) as f64;
    let y_step = cells_y / (ny - 1) as f64;
    let sample_spacing = (x_step * spacing_x).max(y_step * spacing_y) as f32;

    let mut positions = vec![[0.0f32; 3]; nx * ny];
    let mut normals = vec![[0.0f32; 3]; nx * ny];
    let mut uvs = vec![[0.0f32; 2]; nx * ny];

    positions
        .par_chunks_mut(nx)
        .zip(normals.par_chunks_mut(nx))
        .zip(uvs.par_chunks_mut(nx))
        .enumerate()
        .for_each(|(iy, ((row_pos, row_nrm), row_uv))| {
            let gy = iy as f64 * y_step;
            let y = origin_y + gy * spacing_y;
            for ix in 0..nx {
                let gx = ix as f64 * x_step;
                let x = origin_x + gx * spacing_x;
                let base_z = sampler.base_height(x, y);
                let footprint = match view {
                    Some(view) => view.footprint(x, y, base_z, sample_spacing),
                    None => sample_spacing,
                };
                let z = sampler.height(x, y, footprint);
                let normal = sampler.normal(x, y, footprint);
                row_pos[ix] = [x as f32, y as f32, z];
                row_nrm[ix] = normal;
                row_uv[ix] = [(gx / cells_x) as f32, (gy / cells_y) as f32];
            }
        });

    let mut indices = Vec::with_capacity((nx - 1) * (ny - 1) * 2);
    for iy in 0..ny - 1 {
        for ix in 0..nx - 1 {
            let i00 = (iy * nx + ix) as u32;
            let i10 = i00 + 1;
            let i01 = i00 + nx as u32;
            let i11 = i01 + 1;
            indices.push([i00, i10, i01]);
            indices.push([i10, i11, i01]);
        }
    }

    MeshData {
        positions,
        normals,
        uvs,
        indices,
        polygons: Vec::new(),
        nx,
        ny,
        spacing_m: sample_spacing,
        density: 1.0 / step as f32,
    }
}

/// Build a high-density patch around one point, with its border faded to the
/// base surface so it can sit on top of the coarse mesh without a visible seam.
///
/// This is the "walk up to one rock" feature: a 200 m patch at 25 cm spacing
/// costs ~640 k vertices, while the same density over 10 km would be
/// impossible. The patch is lifted by 4 cm to win the depth test against the
/// coarse mesh underneath.
pub fn build_focus_patch(
    sampler: &DetailSampler<'_>,
    center: [f64; 2],
    size_m: f32,
    spacing_m: f32,
    max_vertices: usize,
    view: Option<View>,
) -> MeshData {
    let size = size_m.max(8.0) as f64;
    let mut spacing = spacing_m.max(0.02) as f64;
    let max_vertices = max_vertices.max(64);
    let mut n;
    loop {
        n = (size / spacing).ceil() as usize + 1;
        if n * n <= max_vertices || spacing >= size {
            break;
        }
        spacing *= 1.2;
    }
    n = n.max(2);
    let x0 = center[0] - size * 0.5;
    let y0 = center[1] - size * 0.5;
    let step = size / (n - 1) as f64;
    let sample_spacing = step as f32;
    let edge = (size * 0.10).max(step * 2.0);

    let mut positions = vec![[0.0f32; 3]; n * n];
    let mut normals = vec![[0.0f32; 3]; n * n];
    let mut uvs = vec![[0.0f32; 2]; n * n];
    positions
        .par_chunks_mut(n)
        .zip(normals.par_chunks_mut(n))
        .zip(uvs.par_chunks_mut(n))
        .enumerate()
        .for_each(|(iy, ((row_pos, row_nrm), row_uv))| {
            let y = y0 + iy as f64 * step;
            for ix in 0..n {
                let x = x0 + ix as f64 * step;
                let distance_to_edge = (x - x0)
                    .min(x0 + size - x)
                    .min(y - y0)
                    .min(y0 + size - y)
                    .max(0.0);
                let fade = smoothstep(0.0, edge, distance_to_edge) as f32;
                let base_z = sampler.base_height(x, y);
                let footprint = match view {
                    Some(view) => view.footprint(x, y, base_z, sample_spacing),
                    None => sample_spacing,
                };
                let z = sampler.height_scaled(x, y, footprint, fade) + 0.04;
                let normal = sampler.normal_scaled(x, y, footprint, fade);
                row_pos[ix] = [x as f32, y as f32, z];
                row_nrm[ix] = normal;
                row_uv[ix] = [(ix as f32) / (n - 1) as f32, (iy as f32) / (n - 1) as f32];
            }
        });

    let mut indices = Vec::with_capacity((n - 1) * (n - 1) * 2);
    for iy in 0..n - 1 {
        for ix in 0..n - 1 {
            let i00 = (iy * n + ix) as u32;
            let i10 = i00 + 1;
            let i01 = i00 + n as u32;
            let i11 = i01 + 1;
            indices.push([i00, i10, i01]);
            indices.push([i10, i11, i01]);
        }
    }

    MeshData {
        positions,
        normals,
        uvs,
        indices,
        polygons: Vec::new(),
        nx: n,
        ny: n,
        spacing_m: sample_spacing,
        density: 1.0 / sample_spacing,
    }
}

#[inline]
fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    if (edge1 - edge0).abs() < f64::EPSILON {
        return if x >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::{ClassicParams, DetailSettings, Seed};

    fn sampler<'a>(
        hf: &'a vf_core::HeightField,
        settings: &'a DetailSettings,
    ) -> DetailSampler<'a> {
        DetailSampler::new(hf, settings, Seed::new(5))
    }

    #[test]
    fn subdivision_increases_vertex_count() {
        let hf =
            vf_procgen::classic::generate(&ClassicParams::default().with_size_seed(65, 4)).unwrap();
        let settings = DetailSettings::preset("granite").unwrap();
        let s = sampler(&hf, &settings);
        let a = build(&s, 1, 1_000_000, None);
        let b = build(&s, 2, 1_000_000, None);
        assert_eq!(a.vertex_count(), 65 * 65);
        assert_eq!(b.vertex_count(), 129 * 129);
        assert_eq!(a.triangle_count(), 64 * 64 * 2);
        assert_eq!(b.triangle_count(), 128 * 128 * 2);
    }

    #[test]
    fn vertex_budget_is_respected() {
        let hf = vf_procgen::classic::generate(&ClassicParams::default().with_size_seed(257, 4))
            .unwrap();
        let settings = DetailSettings::preset("basalt").unwrap();
        let s = sampler(&hf, &settings);
        let mesh = build(&s, 4, 50_000, None);
        assert!(mesh.vertex_count() <= 50_000, "{}", mesh.vertex_count());
        assert!(mesh.vertex_count() > 2);
    }

    #[test]
    fn mesh_is_deterministic() {
        let hf =
            vf_procgen::classic::generate(&ClassicParams::default().with_size_seed(65, 4)).unwrap();
        let settings = DetailSettings::preset("limestone").unwrap();
        let s = sampler(&hf, &settings);
        let a = build(&s, 2, 1_000_000, None);
        let b = build(&s, 2, 1_000_000, None);
        assert_eq!(a.positions, b.positions);
        assert_eq!(a.normals, b.normals);
        assert_eq!(a.indices, b.indices);
    }

    #[test]
    fn adaptive_mesh_is_valid_and_deterministic() {
        let hf = vf_procgen::classic::generate(&ClassicParams::default().with_size_seed(257, 4))
            .unwrap();
        let settings = DetailSettings::preset("granite").unwrap();
        let s = sampler(&hf, &settings);
        let view = View {
            camera: [0.0, -2000.0, 3000.0],
            fov_y_rad: 0.9,
            pixel_height: 600,
        };
        let a = build_adaptive(&s, 3, 200_000, view);
        let b = build_adaptive(&s, 3, 200_000, view);
        assert_eq!(a.positions, b.positions);
        assert_eq!(a.indices, b.indices);
        assert!(a.vertex_count() <= 200_000);
        assert!(
            a.indices
                .iter()
                .flatten()
                .all(|i| (*i as usize) < a.vertex_count())
        );
        assert!(
            a.uvs
                .iter()
                .all(|uv| uv[0].is_finite() && uv[1].is_finite())
        );
    }

    #[test]
    fn normals_are_unit_length() {
        let hf =
            vf_procgen::classic::generate(&ClassicParams::default().with_size_seed(33, 4)).unwrap();
        let settings = DetailSettings::preset("volcanic").unwrap();
        let s = sampler(&hf, &settings);
        let mesh = build(&s, 2, 1_000_000, None);
        for n in &mesh.normals {
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-4, "{len}");
        }
    }
}

/// Block size for the adaptive render mesh, in heightfield cells.
const ADAPTIVE_BLOCK: usize = 64;

/// Build a screen-adaptive mesh: each 64-cell block gets a subdivision level
/// from its projected size, and blocks with different levels are stitched with
/// narrow skirts. This is the LOD used by the renderer; export keeps the
/// regular `build` so texture bakes stay on a predictable grid.
pub fn build_adaptive(
    sampler: &DetailSampler<'_>,
    max_subdivision: u32,
    max_vertices: usize,
    view: View,
) -> MeshData {
    let hf = sampler.height_field();
    let (w, h) = (hf.width(), hf.height());
    let cells_x = w - 1;
    let cells_y = h - 1;
    let max_subdivision = max_subdivision.clamp(1, 8) as usize;
    let blocks_x = cells_x.div_ceil(ADAPTIVE_BLOCK);
    let blocks_y = cells_y.div_ceil(ADAPTIVE_BLOCK);
    let (sx, sy) = hf.spacing();
    let (ox, oy) = hf.origin();
    let focal = view.pixel_height.max(1) as f32 / (2.0 * (view.fov_y_rad * 0.5).tan());
    let mut levels = vec![1usize; blocks_x * blocks_y];
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let cx = ox + (bx * ADAPTIVE_BLOCK + ADAPTIVE_BLOCK / 2).min(cells_x) as f64 * sx;
            let cy = oy + (by * ADAPTIVE_BLOCK + ADAPTIVE_BLOCK / 2).min(cells_y) as f64 * sy;
            let distance = ((cx - view.camera[0]).powi(2)
                + (cy - view.camera[1]).powi(2)
                + view.camera[2].powi(2))
            .sqrt()
            .max(1.0) as f32;
            let block_world = (ADAPTIVE_BLOCK as f32 * sx.max(sy) as f32).max(1.0);
            let pixels = block_world / distance * focal;
            let level = (pixels / 18.0).max(1.0).log2().ceil() as usize;
            levels[by * blocks_x + bx] = level.clamp(1, max_subdivision);
        }
    }
    // Budget: coarsen the farthest blocks until the vertex estimate fits.
    let estimate = |levels: &[usize]| -> usize {
        let mut total = 0;
        for by in 0..blocks_y {
            for bx in 0..blocks_x {
                let cw = (ADAPTIVE_BLOCK).min(cells_x - bx * ADAPTIVE_BLOCK);
                let ch = (ADAPTIVE_BLOCK).min(cells_y - by * ADAPTIVE_BLOCK);
                let subdiv = levels[by * blocks_x + bx];
                let nx = cw * subdiv + 1;
                let ny = ch * subdiv + 1;
                // The block plus the four possible skirts (2 verts per edge
                // sample). Without this the budget is exceeded on the first
                // block boundary that changes level.
                total += nx * ny + 2 * (nx + ny);
            }
        }
        total
    };
    while estimate(&levels) > max_vertices.max(1024) {
        let mut best: Option<(usize, f32)> = None;
        for by in 0..blocks_y {
            for bx in 0..blocks_x {
                let index = by * blocks_x + bx;
                if levels[index] <= 1 {
                    continue;
                }
                let cx = ox + (bx * ADAPTIVE_BLOCK + ADAPTIVE_BLOCK / 2) as f64 * sx;
                let cy = oy + (by * ADAPTIVE_BLOCK + ADAPTIVE_BLOCK / 2) as f64 * sy;
                let distance =
                    ((cx - view.camera[0]).powi(2) + (cy - view.camera[1]).powi(2)).sqrt() as f32;
                if best.is_none_or(|(_, d)| distance > d) {
                    best = Some((index, distance));
                }
            }
        }
        let Some((index, _)) = best else { break };
        levels[index] -= 1;
    }

    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    let mut block_vertices: Vec<(usize, usize)> = vec![(0, 0); blocks_x * blocks_y];
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let index = by * blocks_x + bx;
            let subdiv = levels[index];
            let x0 = bx * ADAPTIVE_BLOCK;
            let y0 = by * ADAPTIVE_BLOCK;
            let cw = ADAPTIVE_BLOCK.min(cells_x - x0);
            let ch = ADAPTIVE_BLOCK.min(cells_y - y0);
            let nx = cw * subdiv + 1;
            let ny = ch * subdiv + 1;
            let base = positions.len();
            block_vertices[index] = (base, nx);
            let mut local_pos = vec![[0.0f32; 3]; nx * ny];
            let mut local_nrm = vec![[0.0f32; 3]; nx * ny];
            let mut local_uv = vec![[0.0f32; 2]; nx * ny];
            local_pos
                .par_chunks_mut(nx)
                .zip(local_nrm.par_chunks_mut(nx))
                .zip(local_uv.par_chunks_mut(nx))
                .enumerate()
                .for_each(|(iy, ((row_pos, row_nrm), row_uv))| {
                    let gy = y0 as f64 + iy as f64 / subdiv as f64;
                    let y = oy + gy * sy;
                    for ix in 0..nx {
                        let gx = x0 as f64 + ix as f64 / subdiv as f64;
                        let x = ox + gx * sx;
                        let base_z = sampler.base_height(x, y);
                        let footprint =
                            view.footprint(x, y, base_z, (sx.max(sy) / subdiv as f64) as f32);
                        let z = sampler.height(x, y, footprint);
                        let normal = sampler.normal(x, y, footprint);
                        row_pos[ix] = [x as f32, y as f32, z];
                        row_nrm[ix] = normal;
                        row_uv[ix] = [(gx / cells_x as f64) as f32, (gy / cells_y as f64) as f32];
                    }
                });
            positions.extend_from_slice(&local_pos);
            normals.extend_from_slice(&local_nrm);
            uvs.extend_from_slice(&local_uv);
            for iy in 0..ny - 1 {
                for ix in 0..nx - 1 {
                    let i00 = (base + iy * nx + ix) as u32;
                    let i10 = i00 + 1;
                    let i01 = i00 + nx as u32;
                    let i11 = i01 + 1;
                    indices.push([i00, i10, i01]);
                    indices.push([i10, i11, i01]);
                }
            }
        }
    }

    // Stitch blocks whose neighbour has a different level with narrow
    // vertical skirts. Both sides get a skirt where the level changes, so the
    // gap is covered whichever surface is higher.
    let skirt_depth = (sx.max(sy) as f32 * 2.0).max(2.0);
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let index = by * blocks_x + bx;
            let subdiv = levels[index];
            let (base, nx) = block_vertices[index];
            let y0 = by * ADAPTIVE_BLOCK;
            let ch = ADAPTIVE_BLOCK.min(cells_y - y0);
            let ny = ch * subdiv + 1;
            if bx + 1 < blocks_x && levels[by * blocks_x + bx + 1] != subdiv {
                add_skirt(
                    &mut positions,
                    &mut normals,
                    &mut uvs,
                    &mut indices,
                    base,
                    nx,
                    ny,
                    Side::Right,
                    skirt_depth,
                );
            }
            if bx > 0 && levels[by * blocks_x + bx - 1] != subdiv {
                add_skirt(
                    &mut positions,
                    &mut normals,
                    &mut uvs,
                    &mut indices,
                    base,
                    nx,
                    ny,
                    Side::Left,
                    skirt_depth,
                );
            }
            if by + 1 < blocks_y && levels[(by + 1) * blocks_x + bx] != subdiv {
                add_skirt(
                    &mut positions,
                    &mut normals,
                    &mut uvs,
                    &mut indices,
                    base,
                    nx,
                    ny,
                    Side::Bottom,
                    skirt_depth,
                );
            }
            if by > 0 && levels[(by - 1) * blocks_x + bx] != subdiv {
                add_skirt(
                    &mut positions,
                    &mut normals,
                    &mut uvs,
                    &mut indices,
                    base,
                    nx,
                    ny,
                    Side::Top,
                    skirt_depth,
                );
            }
        }
    }

    MeshData {
        positions,
        normals,
        uvs,
        indices,
        polygons: Vec::new(),
        nx: 0,
        ny: 0,
        spacing_m: sx.max(sy) as f32,
        density: levels.iter().copied().max().unwrap_or(1) as f32,
    }
}

/// Block size for the shape-adaptive mesh, in heightfield cells.
const SHAPE_BLOCK: usize = 32;

/// Build a mesh whose density follows the local geometry, like adaptive layer
/// height in a slicer: each block gets a subdivision level from its relief and
/// the requested tolerance. Flat ground becomes a few large quads; ridges and
/// cliffs receive the vertices.
///
/// Neighbouring blocks are balanced to a 2:1 level ratio and their common
/// edges receive the midpoints of the finer side, so the surface is watertight
/// without skirts. UVs are the global planar world projection, which keeps the
/// texture continuous across every transition.
pub fn build_shape_adaptive<S: SurfaceSampler + ?Sized>(
    sampler: &S,
    max_subdivision: u32,
    tolerance_m: f32,
    max_vertices: usize,
) -> MeshData {
    let hf = sampler.height_field();
    let (width, height) = (hf.width(), hf.height());
    let cells_x = width - 1;
    let cells_y = height - 1;
    let (sx, sy) = hf.spacing();
    let (ox, oy) = hf.origin();
    let blocks_x = cells_x.div_ceil(SHAPE_BLOCK);
    let blocks_y = cells_y.div_ceil(SHAPE_BLOCK);
    let max_level = ((SHAPE_BLOCK as f32 * max_subdivision.max(1) as f32)
        .log2()
        .floor() as usize)
        .clamp(0, 9);
    let tolerance = tolerance_m.clamp(0.02, 100.0);
    // One footprint for every vertex, independent of the block level. Shared
    // block edges must produce exactly the same height on both sides,
    // otherwise the adaptive surface opens hairline cracks that show as black
    // triangles. Sub-cell detail stays in the shader bump.
    let sample_footprint = (sx.max(sy) * 0.5) as f32;
    let mut levels = vec![0u8; blocks_x * blocks_y];

    // The level follows the **deviation from the local plane**, not the raw
    // relief. A smooth 60° slope is planar and stays cheap; a sharp ridge or a
    // pointed face bends away from its corners and is subdivided. This is the
    // "face angle" criterion the triangle forest was missing.
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let x0 = bx * SHAPE_BLOCK;
            let y0 = by * SHAPE_BLOCK;
            let x1 = (x0 + SHAPE_BLOCK).min(cells_x);
            let y1 = (y0 + SHAPE_BLOCK).min(cells_y);
            let sample_at = |u: f64, v: f64| -> f32 {
                let gx = x0 as f64 + (x1 - x0) as f64 * u;
                let gy = y0 as f64 + (y1 - y0) as f64 * v;
                sampler.height(ox + gx * sx, oy + gy * sy, sample_footprint)
            };
            let c00 = sample_at(0.0, 0.0);
            let c10 = sample_at(1.0, 0.0);
            let c01 = sample_at(0.0, 1.0);
            let c11 = sample_at(1.0, 1.0);
            let mut error = 0.0f32;
            for iy in 0..=8 {
                let v = iy as f64 / 8.0;
                for ix in 0..=8 {
                    let u = ix as f64 / 8.0;
                    let z = f64::from(sample_at(u, v));
                    let predicted = f64::from(c00) * (1.0 - u) * (1.0 - v)
                        + f64::from(c10) * u * (1.0 - v)
                        + f64::from(c01) * (1.0 - u) * v
                        + f64::from(c11) * u * v;
                    error = error.max((z - predicted).abs() as f32);
                }
            }
            // Bilinear error of a piecewise-linear patch falls as 1/N²; this
            // turns the tolerance into the number of segments needed.
            let segments = (error / tolerance).max(0.0).sqrt().ceil().max(1.0);
            let level = (segments.log2().ceil() as usize).clamp(0, max_level);
            levels[by * blocks_x + bx] = level as u8;
        }
    }

    // Vertex budget: coarsen the currently densest blocks.
    let estimate = |levels: &[u8]| -> usize {
        levels
            .iter()
            .map(|&level| {
                let n = (1usize << level) + 1;
                n * n
            })
            .sum()
    };
    while estimate(&levels) > max_vertices.max(1024) {
        let mut best: Option<(usize, u8)> = None;
        for (index, &level) in levels.iter().enumerate() {
            if level == 0 {
                continue;
            }
            if best.is_none_or(|(_, current)| level > current) {
                best = Some((index, level));
            }
        }
        let Some((index, _)) = best else { break };
        levels[index] -= 1;
    }

    // 2:1 balance makes every block boundary need at most one midpoint per
    // coarse edge segment.
    loop {
        let mut changed = false;
        for by in 0..blocks_y {
            for bx in 0..blocks_x {
                let index = by * blocks_x + bx;
                let level = levels[index];
                for (nx, ny) in [
                    (bx.wrapping_sub(1), by),
                    (bx + 1, by),
                    (bx, by.wrapping_sub(1)),
                    (bx, by + 1),
                ] {
                    if nx >= blocks_x || ny >= blocks_y {
                        continue;
                    }
                    let neighbour = levels[ny * blocks_x + nx];
                    if neighbour > level + 1 {
                        levels[index] = neighbour - 1;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }

    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    let mut polygons = Vec::new();

    let mut block_origin = vec![0u32; blocks_x * blocks_y];
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let index = by * blocks_x + bx;
            block_origin[index] = positions.len() as u32;
            let level = levels[index] as usize;
            let segments = 1usize << level;
            let n = segments + 1;
            let x0 = bx * SHAPE_BLOCK;
            let y0 = by * SHAPE_BLOCK;
            let x1 = (x0 + SHAPE_BLOCK).min(cells_x);
            let y1 = (y0 + SHAPE_BLOCK).min(cells_y);
            let mut grid = vec![0u32; n * n];

            for iy in 0..n {
                let gy = y0 as f64 + (y1 - y0) as f64 * iy as f64 / segments as f64;
                let y = oy + gy * sy;
                for ix in 0..n {
                    let gx = x0 as f64 + (x1 - x0) as f64 * ix as f64 / segments as f64;
                    let x = ox + gx * sx;
                    let z = sampler.height(x, y, sample_footprint);
                    let normal = sampler.normal(x, y, sample_footprint);
                    grid[iy * n + ix] = positions.len() as u32;
                    positions.push([x as f32, y as f32, z]);
                    normals.push(normal);
                    uvs.push([(gx / cells_x as f64) as f32, (gy / cells_y as f64) as f32]);
                }
            }

            // Midpoints on edges shared with a finer neighbour. Only the
            // edges of this block are decorated; the finer side already has
            // the matching vertices.
            let finer = |nx: usize, ny: usize| -> bool {
                nx < blocks_x && ny < blocks_y && levels[ny * blocks_x + nx] as usize > level
            };
            let mut left_mid = Vec::new();
            let mut right_mid = Vec::new();
            let mut bottom_mid = Vec::new();
            let mut top_mid = Vec::new();
            if bx > 0 && finer(bx - 1, by) {
                for j in 0..segments {
                    let gy = y0 as f64 + (y1 - y0) as f64 * (j as f64 + 0.5) / segments as f64;
                    left_mid.push(push_sample(
                        sampler,
                        (x0 as f64, gy),
                        (ox, oy, sx, sy, cells_x, cells_y),
                        &mut positions,
                        &mut normals,
                        &mut uvs,
                        sample_footprint,
                    ));
                }
            }
            if bx + 1 < blocks_x && finer(bx + 1, by) {
                for j in 0..segments {
                    let gy = y0 as f64 + (y1 - y0) as f64 * (j as f64 + 0.5) / segments as f64;
                    right_mid.push(push_sample(
                        sampler,
                        (x1 as f64, gy),
                        (ox, oy, sx, sy, cells_x, cells_y),
                        &mut positions,
                        &mut normals,
                        &mut uvs,
                        sample_footprint,
                    ));
                }
            }
            if by > 0 && finer(bx, by - 1) {
                for i in 0..segments {
                    let gx = x0 as f64 + (x1 - x0) as f64 * (i as f64 + 0.5) / segments as f64;
                    bottom_mid.push(push_sample(
                        sampler,
                        (gx, y0 as f64),
                        (ox, oy, sx, sy, cells_x, cells_y),
                        &mut positions,
                        &mut normals,
                        &mut uvs,
                        sample_footprint,
                    ));
                }
            }
            if by + 1 < blocks_y && finer(bx, by + 1) {
                for i in 0..segments {
                    let gx = x0 as f64 + (x1 - x0) as f64 * (i as f64 + 0.5) / segments as f64;
                    top_mid.push(push_sample(
                        sampler,
                        (gx, y1 as f64),
                        (ox, oy, sx, sy, cells_x, cells_y),
                        &mut positions,
                        &mut normals,
                        &mut uvs,
                        sample_footprint,
                    ));
                }
            }

            for iy in 0..segments {
                for ix in 0..segments {
                    let i00 = grid[iy * n + ix];
                    let i10 = grid[iy * n + ix + 1];
                    let i11 = grid[(iy + 1) * n + ix + 1];
                    let i01 = grid[(iy + 1) * n + ix];
                    let mut polygon = Vec::with_capacity(6);
                    polygon.push(i00);
                    if iy == 0 && !bottom_mid.is_empty() {
                        polygon.push(bottom_mid[ix]);
                    }
                    polygon.push(i10);
                    if ix == segments - 1 && !right_mid.is_empty() {
                        polygon.push(right_mid[iy]);
                    }
                    polygon.push(i11);
                    if iy == segments - 1 && !top_mid.is_empty() {
                        polygon.push(top_mid[ix]);
                    }
                    polygon.push(i01);
                    if ix == 0 && !left_mid.is_empty() {
                        polygon.push(left_mid[iy]);
                    }
                    for i in 1..polygon.len() - 1 {
                        indices.push([polygon[0], polygon[i], polygon[i + 1]]);
                    }
                    polygons.push(polygon);
                }
            }
        }
    }

    let _ = block_origin;
    MeshData {
        positions,
        normals,
        uvs,
        indices,
        polygons,
        nx: 0,
        ny: 0,
        spacing_m: sx.max(sy) as f32,
        density: levels.iter().copied().max().unwrap_or(0) as f32,
    }
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn push_sample<S: SurfaceSampler + ?Sized>(
    sampler: &S,
    (gx, gy): (f64, f64),
    (ox, oy, sx, sy, cells_x, cells_y): (f64, f64, f64, f64, usize, usize),
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    sample_footprint: f32,
) -> u32 {
    let x = ox + gx * sx;
    let y = oy + gy * sy;
    let z = sampler.height(x, y, sample_footprint);
    let normal = sampler.normal(x, y, sample_footprint);
    let index = positions.len() as u32;
    positions.push([x as f32, y as f32, z]);
    normals.push(normal);
    uvs.push([(gx / cells_x as f64) as f32, (gy / cells_y as f64) as f32]);
    index
}

#[derive(Debug, Clone, Copy)]
enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

#[allow(clippy::too_many_arguments)]
fn add_skirt(
    positions: &mut Vec<[f32; 3]>,
    normals: &mut Vec<[f32; 3]>,
    uvs: &mut Vec<[f32; 2]>,
    indices: &mut Vec<[u32; 3]>,
    base: usize,
    nx: usize,
    ny: usize,
    side: Side,
    depth: f32,
) {
    let count = match side {
        Side::Left | Side::Right => ny,
        Side::Top | Side::Bottom => nx,
    };
    let mut top = Vec::with_capacity(count);
    let mut bottom_indices = Vec::with_capacity(count);
    for i in 0..count {
        let index = match side {
            Side::Right => base + i * nx + nx - 1,
            Side::Left => base + i * nx,
            Side::Top => base + i,
            Side::Bottom => base + (ny - 1) * nx + i,
        };
        let p = positions[index];
        let uv = uvs[index];
        let normal = match side {
            Side::Right => [1.0, 0.0, 0.0],
            Side::Left => [-1.0, 0.0, 0.0],
            Side::Top => [0.0, 1.0, 0.0],
            Side::Bottom => [0.0, -1.0, 0.0],
        };
        let top_index = positions.len() as u32;
        positions.push(p);
        normals.push(normal);
        uvs.push(uv);
        let bottom_index = positions.len() as u32;
        positions.push([p[0], p[1], p[2] - depth]);
        normals.push(normal);
        uvs.push(uv);
        top.push(top_index);
        bottom_indices.push(bottom_index);
    }
    for i in 0..count.saturating_sub(1) {
        indices.push([top[i], top[i + 1], bottom_indices[i]]);
        indices.push([top[i + 1], bottom_indices[i + 1], bottom_indices[i]]);
    }
}

#[cfg(test)]
mod shape_adaptive_tests {
    use super::*;
    use std::collections::HashMap;
    use vf_core::{DetailSettings, HeightField, Seed};

    fn ridge_field() -> HeightField {
        let width = 129;
        let mut heights = vec![0.0f32; width * width];
        for y in 0..width {
            for x in 0..width {
                let dx = (x as f32 - 64.0) / 30.0;
                let dy = (y as f32 - 64.0) / 30.0;
                heights[y * width + x] =
                    120.0 * (-(dx * dx + dy * dy) * 0.5).exp() + (y as f32 * 0.05).sin() * 2.0;
            }
        }
        HeightField::new(width, width, 10.0, 10.0, heights).unwrap()
    }

    #[test]
    fn adaptive_mesh_spends_vertices_on_relief_and_is_watertight() {
        let hf = ridge_field();
        let settings = DetailSettings::default();
        let sampler = DetailSampler::new(&hf, &settings, Seed::new(3));
        let mesh = build_shape_adaptive(&sampler, 1, 4.0, 2_000_000);
        assert!(!mesh.polygons.is_empty());
        assert_eq!(
            mesh.indices.len(),
            mesh.polygons.iter().map(|p| p.len() - 2).sum()
        );
        // Every vertex is inside the UV unit square.
        assert!(
            mesh.uvs
                .iter()
                .all(|uv| uv[0] >= 0.0 && uv[0] <= 1.0 && uv[1] >= 0.0 && uv[1] <= 1.0)
        );
        // Geometric watertightness: merge coincident positions and check that
        // every internal edge is used by exactly two faces, in opposite
        // directions. T-junctions would leave a dangling edge.
        let key = |p: [f32; 3]| -> Key {
            (
                (p[0] * 1000.0).round() as i64,
                (p[1] * 1000.0).round() as i64,
                (p[2] * 1000.0).round() as i64,
            )
        };
        type Key = (i64, i64, i64);
        let mut edges: HashMap<(Key, Key), i32> = HashMap::new();
        for polygon in &mesh.polygons {
            for i in 0..polygon.len() {
                let a = key(mesh.positions[polygon[i] as usize]);
                let b = key(mesh.positions[polygon[(i + 1) % polygon.len()] as usize]);
                *edges.entry((a, b)).or_default() += 1;
            }
        }
        let min_x = mesh
            .positions
            .iter()
            .map(|p| p[0])
            .fold(f32::INFINITY, f32::min);
        let max_x = mesh
            .positions
            .iter()
            .map(|p| p[0])
            .fold(f32::NEG_INFINITY, f32::max);
        let min_y = mesh
            .positions
            .iter()
            .map(|p| p[1])
            .fold(f32::INFINITY, f32::min);
        let max_y = mesh
            .positions
            .iter()
            .map(|p| p[1])
            .fold(f32::NEG_INFINITY, f32::max);
        let on_border = |p: (i64, i64, i64)| {
            let x = p.0 as f32 / 1000.0;
            let y = p.1 as f32 / 1000.0;
            (x - min_x).abs() < 1e-3
                || (x - max_x).abs() < 1e-3
                || (y - min_y).abs() < 1e-3
                || (y - max_y).abs() < 1e-3
        };
        for ((a, b), count) in &edges {
            if on_border(*a) && on_border(*b) {
                // The outer rim of the terrain is a legitimate open boundary.
                continue;
            }
            let reverse = edges.get(&(*b, *a)).copied().unwrap_or(0);
            assert_eq!(
                *count, 1,
                "directed edge used {count} times ({a:?} -> {b:?})"
            );
            assert_eq!(reverse, 1, "edge {a:?} <-> {b:?} is not shared");
        }
    }

    #[test]
    fn detailed_adaptive_mesh_has_no_cracks_on_block_edges() {
        let hf = ridge_field();
        let settings = DetailSettings {
            enabled: true,
            strength: 1.0,
            amplitude_m: 3.0,
            ..DetailSettings::default()
        };
        let sampler = DetailSampler::new(&hf, &settings, Seed::new(5));
        let mesh = build_shape_adaptive(&sampler, 2, 0.8, 2_000_000);
        type Key = (i64, i64, i64);
        let key = |p: [f32; 3]| -> Key {
            (
                (p[0] * 1000.0).round() as i64,
                (p[1] * 1000.0).round() as i64,
                (p[2] * 1000.0).round() as i64,
            )
        };
        let mut edges: std::collections::HashMap<(Key, Key), i32> =
            std::collections::HashMap::new();
        for polygon in &mesh.polygons {
            for i in 0..polygon.len() {
                let a = key(mesh.positions[polygon[i] as usize]);
                let b = key(mesh.positions[polygon[(i + 1) % polygon.len()] as usize]);
                *edges.entry((a, b)).or_default() += 1;
            }
        }
        let min_x = mesh
            .positions
            .iter()
            .map(|p| p[0])
            .fold(f32::INFINITY, f32::min);
        let max_x = mesh
            .positions
            .iter()
            .map(|p| p[0])
            .fold(f32::NEG_INFINITY, f32::max);
        let min_y = mesh
            .positions
            .iter()
            .map(|p| p[1])
            .fold(f32::INFINITY, f32::min);
        let max_y = mesh
            .positions
            .iter()
            .map(|p| p[1])
            .fold(f32::NEG_INFINITY, f32::max);
        let border = |p: Key| {
            let x = p.0 as f32 / 1000.0;
            let y = p.1 as f32 / 1000.0;
            (x - min_x).abs() < 1e-3
                || (x - max_x).abs() < 1e-3
                || (y - min_y).abs() < 1e-3
                || (y - max_y).abs() < 1e-3
        };
        for ((a, b), count) in &edges {
            if border(*a) && border(*b) {
                continue;
            }
            assert_eq!(*count, 1, "edge {a:?} -> {b:?} used {count} times");
            assert_eq!(
                edges.get(&(*b, *a)).copied().unwrap_or(0),
                1,
                "edge {a:?} <-> {b:?} is not shared"
            );
        }
    }

    #[test]
    fn a_planar_slope_is_not_over_subdivided() {
        // A steep but perfectly planar ramp has zero deviation from its
        // corners. The old relief criterion subdivided it to the maximum and
        // produced the "triangle forest"; the plane criterion must keep it
        // coarse.
        let width = 129;
        let heights: Vec<f32> = (0..width * width)
            .map(|index| (index % width) as f32 * 10.0)
            .collect();
        let hf = HeightField::new(width, width, 10.0, 10.0, heights).unwrap();
        let settings = DetailSettings::default();
        let sampler = DetailSampler::new(&hf, &settings, Seed::new(3));
        let mesh = build_shape_adaptive(&sampler, 1, 0.5, 2_000_000);
        // 16 blocks × 4 corners is the floor; even with block-edge midpoints
        // this must stay far below the 16 641-vertex full grid.
        assert!(
            mesh.vertex_count() < 1_000,
            "planar slope was over-subdivided: {} vertices",
            mesh.vertex_count()
        );
    }

    #[test]
    fn tighter_tolerance_creates_more_vertices() {
        let hf = ridge_field();
        let settings = DetailSettings::default();
        let sampler = DetailSampler::new(&hf, &settings, Seed::new(3));
        let coarse = build_shape_adaptive(&sampler, 1, 20.0, 2_000_000);
        let fine = build_shape_adaptive(&sampler, 1, 1.0, 2_000_000);
        assert!(fine.vertex_count() > coarse.vertex_count());
    }
}
