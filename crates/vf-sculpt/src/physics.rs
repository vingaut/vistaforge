//! Deterministic debris physics: broken pieces fall, bounce, slide, pile up
//! and go to sleep, so a collapsed cliff keeps a believable talus at its foot.
//!
//! This is not a general rigid-body engine. It is a fast, stable, deterministic
//! "rock settling" solver: sampled contact points against the terrain BVH,
//! impulse response with restitution and friction, angular damping and sleep.
//! The same seed always produces the same pile, which matters for renders and
//! for the animation module.

use glam::{Quat, Vec3};

use crate::bvh::Bvh;
use crate::fracture::FracturePiece;
use crate::mesh::Mesh;

/// Physics parameters exposed to the UI.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PhysicsSettings {
    pub gravity: f32,
    pub restitution: f32,
    pub friction: f32,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub substeps: u32,
    /// Bodies slower than this for `sleep_time_s` go to sleep.
    pub sleep_speed: f32,
    pub sleep_time_s: f32,
    /// Rock density, kg/m³.
    pub density: f32,
    pub max_time_s: f32,
    /// Contacts are sampled every `stride` vertices.
    pub contact_stride: usize,
}

impl Default for PhysicsSettings {
    fn default() -> Self {
        Self {
            gravity: 9.81,
            restitution: 0.18,
            friction: 0.62,
            linear_damping: 0.06,
            angular_damping: 0.35,
            substeps: 4,
            sleep_speed: 0.06,
            sleep_time_s: 0.4,
            density: 2600.0,
            max_time_s: 20.0,
            contact_stride: 8,
        }
    }
}

impl PhysicsSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.gravity < 0.0 {
            return Err("la gravité doit être positive".into());
        }
        if !(0.0..=1.0).contains(&self.restitution) {
            return Err("la restitution doit être entre 0 et 1".into());
        }
        if self.substeps == 0 || self.substeps > 16 {
            return Err("les sous-pas doivent être entre 1 et 16".into());
        }
        Ok(())
    }
}

/// Static or dynamic body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyKind {
    Dynamic,
    Static,
    /// Dynamic but pulled back toward its rest pose, like a half-detached block.
    Anchored,
}

/// One falling/broken piece.
#[derive(Debug, Clone)]
pub struct DebrisBody {
    pub mesh: Mesh,
    pub position: Vec3,
    pub rotation: Quat,
    pub velocity: Vec3,
    pub angular_velocity: Vec3,
    pub mass_kg: f32,
    pub kind: BodyKind,
    pub restitution: f32,
    pub friction: f32,
    pub sleeping: bool,
    /// Rest position for anchored bodies.
    pub anchor: Vec3,
    pub anchor_stiffness: f32,
    sleep_time: f32,
    pub contacts: u32,
}

impl DebrisBody {
    pub fn new(mesh: Mesh, position: Vec3, kind: BodyKind) -> Self {
        let volume = mesh.volume().abs() as f32;
        Self {
            mesh,
            position,
            rotation: Quat::IDENTITY,
            velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            mass_kg: (volume * 2600.0).max(0.1),
            kind,
            restitution: 0.18,
            friction: 0.62,
            sleeping: kind == BodyKind::Static,
            anchor: position,
            anchor_stiffness: 0.35,
            sleep_time: 0.0,
            contacts: 0,
        }
    }

    pub fn from_piece(piece: &FracturePiece) -> Self {
        // The piece mesh is already in world space; the body transform is
        // relative to its centroid.
        let mut mesh = piece.mesh.clone();
        let center = piece.bounds.center();
        mesh.translate(-center);
        let mut body = Self::new(mesh, center, BodyKind::Dynamic);
        body.mass_kg = (piece.volume_m3 * 2600.0).max(0.1);
        body
    }

    pub fn radius(&self) -> f32 {
        self.mesh.bounds().size().length() * 0.5
    }

    pub fn world_mesh(&self) -> Mesh {
        let mut mesh = self.mesh.clone();
        for position in &mut mesh.positions {
            *position = self.rotation * *position + self.position;
        }
        mesh.recompute_normals();
        mesh
    }

    pub fn kinetic_energy(&self) -> f32 {
        0.5 * self.mass_kg * self.velocity.length_squared()
    }
}

/// A simple spring constraint anchoring a body to a point.
#[derive(Debug, Clone, Copy)]
pub struct AnchorConstraint {
    pub body: usize,
    pub point: Vec3,
    pub stiffness: f32,
    pub damping: f32,
}

/// The simulation.
pub struct PhysicsWorld {
    pub bodies: Vec<DebrisBody>,
    pub settings: PhysicsSettings,
    pub anchors: Vec<AnchorConstraint>,
    time: f32,
    rng: u64,
}

/// What the simulation did.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PhysicsReport {
    pub steps: u32,
    pub simulated_s: f32,
    pub awake: usize,
    pub sleeping: usize,
    pub max_penetration_m: f32,
    pub bounds: crate::math::Aabb,
}

impl PhysicsWorld {
    pub fn new(settings: PhysicsSettings) -> Self {
        Self {
            bodies: Vec::new(),
            settings,
            anchors: Vec::new(),
            time: 0.0,
            rng: 0x9E37_79B9_7F4A_7C15,
        }
    }

    pub fn add(&mut self, body: DebrisBody) -> usize {
        self.bodies.push(body);
        self.bodies.len() - 1
    }

    pub fn add_pieces(&mut self, pieces: &[FracturePiece]) {
        for piece in pieces {
            self.add(DebrisBody::from_piece(piece));
        }
    }

    pub fn time(&self) -> f32 {
        self.time
    }

    /// Advance the simulation. `terrain` supplies the ground height and normal;
    /// without it the bodies fall onto the z = 0 plane.
    pub fn step(&mut self, terrain: Option<(&Mesh, &Bvh)>, dt: f32) -> PhysicsReport {
        let mut report = PhysicsReport::default();
        if dt <= 0.0 {
            return report;
        }
        let substeps = self.settings.substeps.clamp(1, 16);
        let step = (dt / substeps as f32).min(0.02);
        for _ in 0..substeps {
            self.substep(terrain, step, &mut report);
        }
        report.steps = substeps;
        report.simulated_s = dt;
        report.awake = self.bodies.iter().filter(|b| !b.sleeping).count();
        report.sleeping = self.bodies.iter().filter(|b| b.sleeping).count();
        report.max_penetration_m = report.max_penetration_m.min(0.0).abs();
        self.time += dt;
        report
    }

    /// Run until every body sleeps or `max_time_s` is reached.
    pub fn settle(&mut self, terrain: Option<(&Mesh, &Bvh)>, dt: f32) -> PhysicsReport {
        let mut total = PhysicsReport::default();
        let mut elapsed = 0.0;
        while elapsed < self.settings.max_time_s {
            let report = self.step(terrain, dt);
            total.steps += report.steps;
            total.simulated_s += report.simulated_s;
            total.max_penetration_m = total.max_penetration_m.max(report.max_penetration_m);
            elapsed += dt;
            if self.bodies.iter().all(|body| body.sleeping) {
                break;
            }
        }
        total.awake = self.bodies.iter().filter(|b| !b.sleeping).count();
        total.sleeping = self.bodies.iter().filter(|b| b.sleeping).count();
        let mut bounds = crate::math::Aabb::EMPTY;
        for body in &self.bodies {
            bounds = bounds.with_point(body.position - Vec3::splat(body.radius()));
            bounds = bounds.with_point(body.position + Vec3::splat(body.radius()));
        }
        total.bounds = bounds;
        total
    }

    fn substep(&mut self, terrain: Option<(&Mesh, &Bvh)>, dt: f32, report: &mut PhysicsReport) {
        let settings = self.settings;
        for body in &mut self.bodies {
            if body.kind == BodyKind::Static || body.sleeping {
                continue;
            }
            body.velocity.z -= settings.gravity * dt;
            body.velocity *= (1.0 - settings.linear_damping * dt).max(0.0);
            body.position += body.velocity * dt;
            // Integrate the rotation quaternion.
            let omega = body.angular_velocity;
            if omega.length_squared() > 1e-12 {
                let angle = omega.length() * dt;
                let axis = omega.normalize();
                body.rotation = (Quat::from_axis_angle(axis, angle) * body.rotation).normalize();
            }
            body.angular_velocity *= (1.0 - settings.angular_damping * dt).max(0.0);
            body.contacts = 0;
            let radius = body.radius();
            if radius <= 0.0 {
                body.sleeping = true;
                continue;
            }
            // Contact samples: the lowest vertices are the ones that touch
            // first, and a shard often has fewer vertices than the stride.
            let stride = settings.contact_stride.max(1);
            let mut order: Vec<usize> = (0..body.mesh.positions.len()).collect();
            order.sort_by(|a, b| {
                body.mesh.positions[*a]
                    .z
                    .total_cmp(&body.mesh.positions[*b].z)
            });
            let sample_count = (order.len() / stride).max(4).min(order.len());
            let mut deepest = 0.0f32;
            let mut normal_sum = Vec3::ZERO;
            let mut contacts = 0u32;
            for &index in order.iter().take(sample_count) {
                let world = body.rotation * body.mesh.positions[index] + body.position;
                let (ground, normal) = match terrain {
                    Some((mesh, bvh)) => {
                        let height = bvh.height_under(mesh, world.x, world.y).unwrap_or(0.0);
                        let normal = terrain_normal(mesh, bvh, world, height);
                        (height, normal)
                    }
                    None => (0.0, Vec3::Z),
                };
                let penetration = ground - world.z;
                if penetration > 0.0 {
                    contacts += 1;
                    deepest = deepest.max(penetration);
                    normal_sum += normal;
                }
            }
            if contacts == 0 {
                // Sleep check in free fall.
                if body.velocity.length() < settings.sleep_speed {
                    body.sleep_time += dt;
                    if body.sleep_time > settings.sleep_time_s {
                        body.sleeping = true;
                    }
                } else {
                    body.sleep_time = 0.0;
                }
                continue;
            }
            let normal = (normal_sum / contacts as f32).normalize_or_zero();
            let normal = if normal.length_squared() < 0.5 {
                Vec3::Z
            } else {
                normal
            };
            body.position += normal * deepest.max(0.0);
            let normal_speed = body.velocity.dot(normal);
            if normal_speed < 0.0 {
                body.velocity -= normal * (1.0 + body.restitution) * normal_speed;
            }
            let tangential = body.velocity - normal * body.velocity.dot(normal);
            let friction = body.friction.clamp(0.0, 1.0) * settings.friction.max(0.0);
            body.velocity -= tangential * friction.min(0.95);
            // Rolling and tumbling: a tangential velocity at the contact
            // radius turns into angular velocity.
            let contact_offset = normal * radius;
            body.angular_velocity += contact_offset.cross(tangential) * (0.5 / radius.max(0.1));
            body.angular_velocity *= 0.86;
            if body.velocity.length() < settings.sleep_speed
                && body.angular_velocity.length() < settings.sleep_speed * 2.0
            {
                body.sleep_time += dt;
                if body.sleep_time > settings.sleep_time_s {
                    body.sleeping = true;
                    body.velocity = Vec3::ZERO;
                    body.angular_velocity = Vec3::ZERO;
                }
            } else {
                body.sleep_time = 0.0;
            }
            body.contacts = contacts;
            report.max_penetration_m = report.max_penetration_m.max(deepest);
        }
        // Anchored bodies are pulled back toward their rest pose.
        let anchors = self.anchors.clone();
        for anchor in anchors {
            if let Some(body) = self.bodies.get_mut(anchor.body) {
                if body.kind != BodyKind::Anchored || body.sleeping {
                    continue;
                }
                let delta = anchor.point - body.position;
                body.velocity += delta * anchor.stiffness * dt;
                body.velocity *= (1.0 - anchor.damping * dt).max(0.0);
            }
        }
    }

    /// Apply an outward impulse to every body within `radius` of `center`.
    pub fn impulse(&mut self, center: Vec3, radius: f32, strength: f32) {
        let mut rng = self.rng;
        let mut next = || {
            rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
            (rng >> 40) as f32 / (1u64 << 24) as f32
        };
        for body in &mut self.bodies {
            let offset = body.position - center;
            let distance = offset.length();
            if distance < radius && distance > 1e-4 {
                let falloff = 1.0 - (distance / radius).clamp(0.0, 1.0);
                let direction = (offset / distance + Vec3::Z * 0.6).normalize();
                body.velocity += direction * strength * falloff;
                body.angular_velocity += Vec3::new(
                    (next() - 0.5) * strength * falloff,
                    (next() - 0.5) * strength * falloff,
                    (next() - 0.5) * strength * falloff,
                );
                body.sleeping = false;
                body.sleep_time = 0.0;
            }
        }
        self.rng = rng;
    }

    pub fn total_mass_kg(&self) -> f32 {
        self.bodies.iter().map(|b| b.mass_kg).sum()
    }

    /// Combined mesh of every body at its current transform, for export.
    pub fn merged_mesh(&self) -> Mesh {
        let mut out = Mesh::new();
        for body in &self.bodies {
            let mesh = body.world_mesh();
            out.merge(&mesh);
        }
        out
    }
}

fn terrain_normal(mesh: &Mesh, bvh: &Bvh, world: Vec3, height: f32) -> Vec3 {
    let _ = (mesh, bvh);
    // A central-difference normal of the height field through the BVH.
    let epsilon = 0.35;
    let (terrain, tree) = (mesh, bvh);
    let hx = tree
        .height_under(terrain, world.x + epsilon, world.y)
        .unwrap_or(height);
    let hx0 = tree
        .height_under(terrain, world.x - epsilon, world.y)
        .unwrap_or(height);
    let hy = tree
        .height_under(terrain, world.x, world.y + epsilon)
        .unwrap_or(height);
    let hy0 = tree
        .height_under(terrain, world.x, world.y - epsilon)
        .unwrap_or(height);
    Vec3::new(
        -(hx - hx0) / (2.0 * epsilon),
        -(hy - hy0) / (2.0 * epsilon),
        1.0,
    )
    .normalize_or_zero()
}

/// Generate a small shard mesh for a debris burst, deterministic from `seed`.
pub fn shard(size_m: f32, seed: u64) -> Mesh {
    let mut positions = Vec::new();
    let base = Vec3::ZERO;
    let mut rng = seed | 1;
    let mut next = || {
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (rng >> 40) as f32 / (1u64 << 24) as f32
    };
    // Five-sided irregular pyramid: a broken rock chip, not a perfect die.
    let sides = 5;
    let mut ring = Vec::new();
    for i in 0..sides {
        let angle = std::f32::consts::TAU * i as f32 / sides as f32;
        let radius = size_m * (0.5 + next() * 0.5);
        ring.push(base + Vec3::new(angle.cos() * radius, angle.sin() * radius, 0.0));
    }
    let apex = base + Vec3::new(0.0, 0.0, size_m * (0.6 + next() * 0.6));
    positions.push(apex);
    for p in &ring {
        positions.push(*p);
    }
    positions.push(base - Vec3::new(0.0, 0.0, size_m * 0.2));
    let apex_index = 0u32;
    let ring_start = 1u32;
    let bottom = 1 + sides as u32;
    let mut faces = Vec::new();
    for i in 0..sides as u32 {
        faces.push(vec![
            apex_index,
            ring_start + i,
            ring_start + (i + 1) % sides as u32,
        ]);
        faces.push(vec![
            bottom,
            ring_start + (i + 1) % sides as u32,
            ring_start + i,
        ]);
    }
    let mut mesh = Mesh::from_faces(positions, &faces);
    mesh.recompute_normals();
    mesh
}

/// Build a debris field of `count` shards around `center`.
pub fn debris_shards(
    center: Vec3,
    radius: f32,
    count: u32,
    size_m: f32,
    seed: u64,
) -> Vec<DebrisBody> {
    let mut bodies = Vec::new();
    let mut rng = seed | 1;
    let mut next = || {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        (rng >> 40) as f32 / (1u64 << 24) as f32
    };
    for _ in 0..count {
        let angle = next() * std::f32::consts::TAU;
        let distance = radius * next().sqrt();
        let position =
            center + Vec3::new(angle.cos() * distance, angle.sin() * distance, radius * 0.4);
        let shard = shard(size_m * (0.35 + next() * 0.9), seed ^ (bodies.len() as u64));
        let mut body = DebrisBody::new(shard, position, BodyKind::Dynamic);
        body.velocity = Vec3::new(
            (next() - 0.5) * 3.0,
            (next() - 0.5) * 3.0,
            1.0 + next() * 4.0,
        );
        body.angular_velocity = Vec3::new(
            (next() - 0.5) * 2.0,
            (next() - 0.5) * 2.0,
            (next() - 0.5) * 2.0,
        );
        bodies.push(body);
    }
    bodies
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::Mesh;

    fn ground() -> Mesh {
        let mut positions = Vec::new();
        for y in 0..=4 {
            for x in 0..=4 {
                positions.push(Vec3::new(x as f32 * 5.0 - 10.0, y as f32 * 5.0 - 10.0, 0.0));
            }
        }
        let mut faces = Vec::new();
        for y in 0..4u32 {
            for x in 0..4u32 {
                let a = y * 5 + x;
                faces.push(vec![a, a + 1, a + 6, a + 5]);
            }
        }
        let mut mesh = Mesh::from_faces(positions, &faces);
        mesh.recompute_normals();
        mesh
    }

    #[test]
    fn shards_fall_and_settle_on_the_ground() {
        let mesh = ground();
        let bvh = Bvh::build(&mesh);
        let mut world = PhysicsWorld::new(PhysicsSettings::default());
        for shard in debris_shards(Vec3::new(0.0, 0.0, 5.0), 3.0, 8, 0.6, 3) {
            world.add(shard);
        }
        let report = world.settle(Some((&mesh, &bvh)), 1.0 / 60.0);
        assert!(report.steps > 0);
        assert!(
            world.bodies.iter().all(|b| b.position.z > -0.3),
            "{:?}",
            world.bodies.first().map(|b| b.position)
        );
        assert!(world.bodies.iter().any(|b| b.sleeping));
    }

    #[test]
    fn an_impulse_wakes_and_moves_a_pile() {
        let mesh = ground();
        let bvh = Bvh::build(&mesh);
        let mut world = PhysicsWorld::new(PhysicsSettings::default());
        for shard in debris_shards(Vec3::ZERO, 2.0, 4, 0.5, 11) {
            world.add(shard);
        }
        world.settle(Some((&mesh, &bvh)), 1.0 / 60.0);
        let before: Vec<Vec3> = world.bodies.iter().map(|b| b.position).collect();
        world.impulse(Vec3::ZERO, 10.0, 6.0);
        world.step(Some((&mesh, &bvh)), 1.0 / 60.0);
        assert!(
            world
                .bodies
                .iter()
                .zip(before.iter())
                .any(|(b, p)| b.position.distance(*p) > 1e-4)
        );
    }

    #[test]
    fn shards_are_deterministic() {
        let a = debris_shards(Vec3::ZERO, 2.0, 5, 0.5, 7);
        let b = debris_shards(Vec3::ZERO, 2.0, 5, 0.5, 7);
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.position, y.position);
            assert_eq!(x.mesh.positions, y.mesh.positions);
        }
    }

    #[test]
    fn a_body_does_not_sink_through_the_ground() {
        let mesh = ground();
        let bvh = Bvh::build(&mesh);
        let mut world = PhysicsWorld::new(PhysicsSettings::default());
        let body = DebrisBody::new(shard(1.0, 1), Vec3::new(0.0, 0.0, 2.0), BodyKind::Dynamic);
        world.add(body);
        world.settle(Some((&mesh, &bvh)), 1.0 / 60.0);
        let body = &world.bodies[0];
        assert!(body.position.z > -0.2, "{}", body.position.z);
    }
}
