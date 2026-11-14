//! Tiled triangle rasterisation.

use glam::Vec3;

use crate::{
    KIND_GROUND, KIND_SKIRT, KIND_WATER, SceneData, ShadeCtx, TILE, Vertex, ray_dir_for_pixel,
};

pub(crate) struct TileJob {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
    tris: Vec<u32>,
}

pub(crate) struct TileOut {
    x0: usize,
    y0: usize,
    w: usize,
    h: usize,
    color: Vec<[f32; 3]>,
}

#[derive(Clone, Copy)]
struct ScreenVertex {
    x: f32,
    y: f32,
    inv_w: f32,
    pos: Vec3,
    normal: Vec3,
    ao: f32,
}

pub(crate) fn build_jobs(scene: &SceneData, width: usize, height: usize) -> Vec<TileJob> {
    let tiles_x = width.div_ceil(TILE);
    let tiles_y = height.div_ceil(TILE);
    let mut jobs: Vec<TileJob> = Vec::with_capacity(tiles_x * tiles_y);
    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            jobs.push(TileJob {
                x0: tx * TILE,
                y0: ty * TILE,
                x1: ((tx + 1) * TILE).min(width),
                y1: ((ty + 1) * TILE).min(height),
                tris: Vec::new(),
            });
        }
    }

    let near = scene.near;
    for (index, tri) in scene.triangles.iter().enumerate() {
        let v0 = scene.vertices[tri.a as usize];
        let v1 = scene.vertices[tri.b as usize];
        let v2 = scene.vertices[tri.c as usize];
        if v0.clip.w < near && v1.clip.w < near && v2.clip.w < near {
            continue;
        }
        let crosses_near = v0.clip.w < near || v1.clip.w < near || v2.clip.w < near;
        let (min_x, min_y, max_x, max_y) = if crosses_near {
            (0usize, 0usize, width, height)
        } else {
            let p0 = project(v0, width, height);
            let p1 = project(v1, width, height);
            let p2 = project(v2, width, height);
            let min_x = p0.x.min(p1.x).min(p2.x).floor().max(0.0) as usize;
            let min_y = p0.y.min(p1.y).min(p2.y).floor().max(0.0) as usize;
            let max_x = p0.x.max(p1.x).max(p2.x).ceil().min(width as f32) as usize;
            let max_y = p0.y.max(p1.y).max(p2.y).ceil().min(height as f32) as usize;
            if min_x >= max_x || min_y >= max_y {
                continue;
            }
            (min_x, min_y, max_x, max_y)
        };

        let tx0 = min_x / TILE;
        let tx1 = (max_x.saturating_sub(1)) / TILE;
        let ty0 = min_y / TILE;
        let ty1 = (max_y.saturating_sub(1)) / TILE;
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                jobs[ty * tiles_x + tx].tris.push(index as u32);
            }
        }
    }
    jobs
}

pub(crate) fn render_tile(
    job: TileJob,
    scene: &SceneData,
    ctx: &ShadeCtx<'_>,
    width: usize,
    height: usize,
) -> TileOut {
    let w = job.x1 - job.x0;
    let h = job.y1 - job.y0;
    let mut color = vec![[0.0f32; 3]; w * h];
    let mut depth = vec![0.0f32; w * h];

    for y in 0..h {
        for x in 0..w {
            let dir = ray_dir_for_pixel(scene, job.x0 + x, job.y0 + y, width, height);
            color[y * w + x] = ctx.sky(dir);
        }
    }

    for &tri_index in &job.tris {
        let tri = scene.triangles[tri_index as usize];
        let vs = [
            scene.vertices[tri.a as usize],
            scene.vertices[tri.b as usize],
            scene.vertices[tri.c as usize],
        ];
        if vs[0].clip.w < scene.near && vs[1].clip.w < scene.near && vs[2].clip.w < scene.near {
            continue;
        }
        let crosses =
            vs[0].clip.w < scene.near || vs[1].clip.w < scene.near || vs[2].clip.w < scene.near;
        if crosses {
            let polygon = clip_near(vs, scene.near);
            for i in 1..polygon.len().saturating_sub(1) {
                let a = project(polygon[0], width, height);
                let b = project(polygon[i], width, height);
                let c = project(polygon[i + 1], width, height);
                raster(a, b, c, tri.kind, &job, &mut color, &mut depth, w, ctx);
            }
        } else {
            let a = project(vs[0], width, height);
            let b = project(vs[1], width, height);
            let c = project(vs[2], width, height);
            raster(a, b, c, tri.kind, &job, &mut color, &mut depth, w, ctx);
        }
    }

    TileOut {
        x0: job.x0,
        y0: job.y0,
        w,
        h,
        color,
    }
}

#[allow(clippy::too_many_arguments)]
fn raster(
    a: ScreenVertex,
    b: ScreenVertex,
    c: ScreenVertex,
    kind: u8,
    job: &TileJob,
    color: &mut [[f32; 3]],
    depth: &mut [f32],
    tile_width: usize,
    ctx: &ShadeCtx<'_>,
) {
    let area = edge(a.x, a.y, b.x, b.y, c.x, c.y);
    if area.abs() < 1e-8 {
        return;
    }
    let (p0, p1, p2) = if area > 0.0 { (a, b, c) } else { (a, c, b) };
    let min_x = p0.x.min(p1.x).min(p2.x).floor().max(job.x0 as f32);
    let max_x = p0.x.max(p1.x).max(p2.x).ceil().min(job.x1 as f32);
    let min_y = p0.y.min(p1.y).min(p2.y).floor().max(job.y0 as f32);
    let max_y = p0.y.max(p1.y).max(p2.y).ceil().min(job.y1 as f32);
    if min_x >= max_x || min_y >= max_y {
        return;
    }
    let x_start = min_x as usize;
    let x_end = max_x as usize;
    let y_start = min_y as usize;
    let y_end = max_y as usize;

    for y in y_start..y_end {
        let fy = y as f32 + 0.5;
        let local_y = y - job.y0;
        for x in x_start..x_end {
            let fx = x as f32 + 0.5;
            let w0 = edge(p1.x, p1.y, p2.x, p2.y, fx, fy);
            let w1 = edge(p2.x, p2.y, p0.x, p0.y, fx, fy);
            let w2 = edge(p0.x, p0.y, p1.x, p1.y, fx, fy);
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let sum = w0 + w1 + w2;
            if sum <= 1e-9 {
                continue;
            }
            let inv_w = (w0 * p0.inv_w + w1 * p1.inv_w + w2 * p2.inv_w) / sum;
            let local_x = x - job.x0;
            let idx = local_y * tile_width + local_x;
            if inv_w <= depth[idx] {
                continue;
            }
            let inv_w_raw = w0 * p0.inv_w + w1 * p1.inv_w + w2 * p2.inv_w;
            if inv_w_raw <= 1e-12 {
                continue;
            }
            let pos =
                (p0.pos * (w0 * p0.inv_w) + p1.pos * (w1 * p1.inv_w) + p2.pos * (w2 * p2.inv_w))
                    / inv_w_raw;
            let normal = (p0.normal * (w0 * p0.inv_w)
                + p1.normal * (w1 * p1.inv_w)
                + p2.normal * (w2 * p2.inv_w))
                / inv_w_raw;
            let ao = (p0.ao * (w0 * p0.inv_w) + p1.ao * (w1 * p1.inv_w) + p2.ao * (w2 * p2.inv_w))
                / inv_w_raw;
            let normal = normal.normalize_or_zero();
            let normal = if normal.length_squared() < 0.5 {
                Vec3::Z
            } else {
                normal
            };

            let shaded = if kind == KIND_WATER {
                match ctx.water(pos) {
                    Some(c) => c,
                    None => continue,
                }
            } else if kind == KIND_SKIRT {
                ctx.skirt(pos, normal)
            } else if kind == KIND_GROUND {
                ctx.ground(pos, normal)
            } else {
                ctx.terrain(pos, normal, ao.clamp(0.0, 1.0))
            };
            depth[idx] = inv_w;
            color[idx] = shaded;
        }
    }
}

#[inline]
fn edge(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (bx - ax) * (py - ay) - (by - ay) * (px - ax)
}

fn project(v: Vertex, width: usize, height: usize) -> ScreenVertex {
    let inv_w = 1.0 / v.clip.w;
    let ndc_x = v.clip.x * inv_w;
    let ndc_y = v.clip.y * inv_w;
    ScreenVertex {
        x: (ndc_x * 0.5 + 0.5) * width as f32,
        y: (1.0 - (ndc_y * 0.5 + 0.5)) * height as f32,
        inv_w,
        pos: v.pos,
        normal: v.normal,
        ao: v.ao,
    }
}

fn clip_near(vs: [Vertex; 3], near: f32) -> Vec<Vertex> {
    let mut out = Vec::with_capacity(4);
    for i in 0..3 {
        let a = vs[i];
        let b = vs[(i + 1) % 3];
        let a_in = a.clip.w >= near;
        let b_in = b.clip.w >= near;
        if a_in {
            out.push(a);
        }
        if a_in != b_in {
            let denom = b.clip.w - a.clip.w;
            if denom.abs() > 1e-12 {
                let t = (near - a.clip.w) / denom;
                out.push(lerp_vertex(a, b, t));
            }
        }
    }
    out
}

fn lerp_vertex(a: Vertex, b: Vertex, t: f32) -> Vertex {
    Vertex {
        pos: a.pos.lerp(b.pos, t),
        normal: a.normal.lerp(b.normal, t),
        ao: a.ao + (b.ao - a.ao) * t,
        clip: a.clip.lerp(b.clip, t),
    }
}

pub(crate) fn blit_tile(dst: &mut [[f32; 3]], dst_width: usize, tile: &TileOut) {
    for y in 0..tile.h {
        let src = &tile.color[y * tile.w..(y + 1) * tile.w];
        let start = (tile.y0 + y) * dst_width + tile.x0;
        dst[start..start + tile.w].copy_from_slice(src);
    }
}
