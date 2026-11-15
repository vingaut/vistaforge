//! Ground and wall safety layer for the camera rig.
//!
//! The authored curve remains the timing reference, but it can pass through a
//! hill or a cliff. This layer computes a *safe* target for each simulation
//! step:
//!
//! * a smoothed, anticipation-based ground envelope with a hard floor;
//! * a look-ahead wall detector that either climbs over a wall when the body
//!   can do it, or generates a lateral detour and returns to the authored path.
//!
//! It is deliberately independent from the motion profile: it works in
//! `exact_path` mode too, where the profile normally cannot move the eye.

use glam::{DVec2, DVec3};
use vf_core::{CameraRig, Constraints, HeightField, MotionProfile, Path3D};

use crate::clearance::ClearanceField;
use crate::rig::sample_path;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SafetyReport {
    pub ground_active: bool,
    pub wall_active: bool,
    pub wall_climb: bool,
    pub blocked: bool,
    pub wall_side: i8,
    pub correction_m: f32,
}

#[derive(Debug, Clone, Copy)]
struct WallHit {
    point: DVec3,
    time_s: f32,
    outward: DVec2,
    top: f32,
}

#[derive(Debug, Clone)]
pub struct SafetyState {
    /// Smoothed ground floor (terrain + clearance).
    floor_smooth: f64,
    /// Current lateral detour offset, world X/Y.
    offset: DVec2,
    offset_vel: DVec2,
    wall_active: bool,
    /// A climb decision is sticky while the wall is in view; otherwise the
    /// `can_climb` test changes as the camera approaches and the guard flips
    /// between climbing and detouring.
    wall_climb_active: bool,
    wall_side: f64,
    /// Last safe target, used to derive an honest velocity.
    last: Option<DVec3>,
    last_velocity: DVec3,
    report: SafetyReport,
}

impl SafetyState {
    pub fn new(rig: &CameraRig, hf: &HeightField) -> Self {
        let eye = sample_path(&rig.eye, 0.0);
        let floor = hard_floor(hf, &rig.constraints, eye.x, eye.y);
        Self {
            floor_smooth: floor,
            offset: DVec2::ZERO,
            offset_vel: DVec2::ZERO,
            wall_active: false,
            wall_climb_active: false,
            wall_side: 1.0,
            last: None,
            last_velocity: DVec3::ZERO,
            report: SafetyReport::default(),
        }
    }

    pub fn reset(&mut self, rig: &CameraRig, hf: &HeightField) {
        *self = Self::new(rig, hf);
    }

    pub fn report(&self) -> SafetyReport {
        self.report
    }

    /// Apply ground and wall safety to the authored target at `time`.
    ///
    /// Returns `(safe_position, actual_velocity)`. The caller may feed the
    /// safe position to the exact sampler or to the profile spring.
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &mut self,
        hf: &HeightField,
        clearance: &ClearanceField,
        constraints: &Constraints,
        body: &MotionProfile,
        path: &Path3D,
        time: f32,
        desired: DVec3,
        velocity: DVec3,
        dt: f64,
    ) -> (DVec3, DVec3) {
        let mut safe = desired;
        let mut report = SafetyReport::default();
        let dt = dt.max(1e-5);

        let wall_enabled =
            constraints.enabled && constraints.wall_avoidance && constraints.wall_distance_m > 0.0;
        let hit = if wall_enabled {
            find_wall_hit(clearance, constraints, path, time, dt)
        } else {
            None
        };
        if hit.is_none() {
            self.wall_climb_active = false;
        }
        if let Some(hit) = &hit
            && !self.wall_active
            && !self.wall_climb_active
        {
            self.wall_climb_active = can_climb(body, constraints, hit, time, velocity);
        }
        let climb = hit.is_some() && self.wall_climb_active;
        let detour = hit.is_some() && !climb;

        if constraints.enabled && constraints.ground_guard {
            // During a lateral detour, do not anticipate the wall with the
            // ground envelope: otherwise the guard would "climb" the wall
            // instead of going around it. The current footprint is still a
            // hard floor.
            let target = if detour {
                hard_floor(hf, constraints, desired.x, desired.y)
            } else {
                ground_target(hf, constraints, desired, velocity)
            };
            self.floor_smooth = smooth_floor(self.floor_smooth, target, dt, constraints.softness);
            safe.z = safe.z.max(self.floor_smooth);
            let hard = hard_floor(hf, constraints, safe.x, safe.y);
            safe.z = safe.z.max(hard);
            report.ground_active = safe.z > desired.z + 1e-3;
        } else {
            self.floor_smooth = hard_floor(hf, constraints, desired.x, desired.y);
        }

        if let Some(hit) = hit {
            if climb {
                let climb = f64::from(hit.top + constraints.wall_vertical_margin_m);
                self.floor_smooth = smooth_floor(
                    self.floor_smooth.max(safe.z),
                    climb,
                    dt,
                    constraints.softness,
                );
                safe.z = safe.z.max(self.floor_smooth);
                report.wall_climb = true;
                // Let a previous detour relax while climbing.
                self.drive_offset(DVec2::ZERO, body, dt);
            } else {
                self.drive_detour(clearance, constraints, body, &hit, velocity, dt);
                safe.x += self.offset.x;
                safe.y += self.offset.y;
                if constraints.ground_guard {
                    let hard = hard_floor(hf, constraints, safe.x, safe.y);
                    safe.z = safe.z.max(hard);
                }
                report.wall_active = self.wall_active;
                report.wall_side = if self.wall_side >= 0.0 { 1 } else { -1 };
                // Hard fallback: never let the final position be inside a
                // wall even if the corridor is too narrow.
                let margin = f64::from(constraints.wall_distance_m + constraints.wall_margin_m);
                let vertical = f64::from(constraints.wall_vertical_margin_m);
                let (distance, gx, gy, top) = clearance.sample(safe.x, safe.y);
                if distance.is_finite()
                    && f64::from(distance) < margin
                    && safe.z < f64::from(top) + vertical
                {
                    // The tangent corridor still intersects the wall (for
                    // example an infinite cliff). Push away along the outward
                    // normal by the missing distance before using the vertical
                    // escape.
                    let outward = DVec2::new(f64::from(gx), f64::from(gy)).normalize_or_zero();
                    if outward.length_squared() > 0.5 {
                        let missing = margin - f64::from(distance) + 1.0;
                        safe.x += outward.x * missing;
                        safe.y += outward.y * missing;
                        self.offset += outward * missing;
                    }
                    let (distance, _, _, top) = clearance.sample(safe.x, safe.y);
                    if distance.is_finite()
                        && f64::from(distance) < margin
                        && safe.z < f64::from(top) + vertical
                    {
                        safe.z = f64::from(top) + vertical;
                        report.blocked = true;
                    }
                }
            }
        } else {
            self.drive_offset(DVec2::ZERO, body, dt);
            if self.offset.length() < 0.05 && self.offset_vel.length() < 0.05 {
                self.wall_active = false;
            }
        }

        enforce_hard(hf, clearance, constraints, &mut safe);
        report.correction_m = (safe - desired).length() as f32;
        self.report = report;

        // Honest velocity for target look-ahead and speed readout. A first
        // order filter avoids spikes when the hard floor is hit.
        if let Some(last) = self.last {
            let raw = (safe - last) / dt;
            let alpha = (dt * 20.0).clamp(0.05, 1.0);
            self.last_velocity = self.last_velocity.lerp(raw, alpha);
        } else {
            self.last_velocity = velocity;
        }
        let max_velocity = f64::from(body.max_speed_mps.max(1.0)) * 2.0;
        if self.last_velocity.length() > max_velocity {
            self.last_velocity = self.last_velocity.normalize_or_zero() * max_velocity;
        }
        self.last = Some(safe);
        (safe, self.last_velocity)
    }

    /// Hard safety clamp for an arbitrary position (camera, target, inertial
    /// eye after the spring). No smoothing: this is the last line of defence.
    pub fn enforce(
        &self,
        hf: &HeightField,
        clearance: &ClearanceField,
        constraints: &Constraints,
        point: &mut DVec3,
    ) {
        enforce_hard(hf, clearance, constraints, point);
    }

    fn drive_detour(
        &mut self,
        clearance: &ClearanceField,
        constraints: &Constraints,
        body: &MotionProfile,
        hit: &WallHit,
        velocity: DVec3,
        dt: f64,
    ) {
        let tangent = DVec2::new(-hit.outward.y, hit.outward.x);
        let inward = -hit.outward;
        let direction = DVec2::new(velocity.x, velocity.y);
        let direction = if direction.length_squared() > 1e-6 {
            direction.normalize()
        } else {
            inward
        };
        let impact = direction.dot(inward).clamp(-1.0, 1.0).acos().to_degrees();

        if !self.wall_active {
            let detour = detour_distance(constraints);
            let mut best_side = 1.0f64;
            let mut best_score = f64::NEG_INFINITY;
            for side in [-1.0f64, 1.0] {
                let probe = DVec2::new(
                    hit.point.x + tangent.x * side * detour,
                    hit.point.y + tangent.y * side * detour,
                );
                let (distance, _, _, _) = clearance.sample(probe.x, probe.y);
                let score = if distance.is_finite() {
                    f64::from(distance)
                } else {
                    1.0e6
                };
                if score > best_score {
                    best_score = score;
                    best_side = side;
                }
            }
            self.wall_side = best_side;
            self.wall_active = true;
        }

        let detour = detour_distance(constraints);
        let target = if impact <= f64::from(constraints.wall_angle_deg) {
            tangent * self.wall_side * detour
        } else {
            hit.outward * detour
        };
        self.drive_offset(target, body, dt);
    }

    fn drive_offset(&mut self, target: DVec2, body: &MotionProfile, dt: f64) {
        let k: f64 = 7.0;
        let damping = 2.0 * k.sqrt() * 0.9;
        let mut accel = (target - self.offset) * k - self.offset_vel * damping;
        let max_accel = f64::from(body.accel_lateral_mps2.max(0.5)).clamp(15.0, 40.0);
        if accel.length() > max_accel {
            accel = accel.normalize_or_zero() * max_accel;
        }
        self.offset_vel += accel * dt;
        let max_speed = f64::from(body.max_speed_mps.max(1.0)).max(30.0) * 0.8;
        if self.offset_vel.length() > max_speed {
            self.offset_vel = self.offset_vel.normalize_or_zero() * max_speed;
        }
        self.offset += self.offset_vel * dt;
        let max_offset = f64::from(body.max_speed_mps.max(1.0)) * 4.0 + 100.0;
        if self.offset.length() > max_offset {
            self.offset = self.offset.normalize_or_zero() * max_offset;
        }
    }
}

fn find_wall_hit(
    clearance: &ClearanceField,
    constraints: &Constraints,
    path: &Path3D,
    time: f32,
    _dt: f64,
) -> Option<WallHit> {
    let look = constraints.wall_look_ahead_s.max(0.2);
    let margin = f64::from(constraints.wall_distance_m + constraints.wall_margin_m);
    let vertical = f64::from(constraints.wall_vertical_margin_m);
    let steps = 12;
    for index in 1..=steps {
        let fraction = index as f32 / steps as f32;
        let sample_time = (time + look * fraction).min(path.duration_s().max(time + 1e-4));
        if sample_time <= time + 1e-4 {
            continue;
        }
        let point = sample_path(path, sample_time);
        let (distance, gx, gy, top) = clearance.sample(point.x, point.y);
        if !distance.is_finite() || !f64::from(top).is_finite() {
            continue;
        }
        if f64::from(distance) >= margin {
            continue;
        }
        if point.z >= f64::from(top) + vertical {
            continue;
        }
        let outward = DVec2::new(f64::from(gx), f64::from(gy)).normalize_or_zero();
        if outward.length_squared() < 0.5 {
            continue;
        }
        return Some(WallHit {
            point,
            time_s: sample_time,
            outward,
            top,
        });
    }
    None
}

fn can_climb(
    body: &MotionProfile,
    constraints: &Constraints,
    hit: &WallHit,
    time: f32,
    velocity: DVec3,
) -> bool {
    let required = f64::from(hit.top + constraints.wall_vertical_margin_m) - hit.point.z;
    if required <= 0.0 {
        return true;
    }
    let time_to = f64::from((hit.time_s - time).max(0.15));
    let accel = f64::from(body.accel_vertical_mps2.max(0.1));
    let vz = velocity.z.max(0.0);
    let available = 0.5 * accel * time_to * time_to + vz * time_to;
    // Keep a margin: an aggressive climb that only just fits is uncomfortable
    // and visually brutal, so prefer the lateral detour.
    required <= available * 0.8
}

fn detour_distance(constraints: &Constraints) -> f64 {
    let base = f64::from(constraints.wall_distance_m + constraints.wall_margin_m + 20.0);
    if constraints.wall_detour_m > 0.0 {
        base.max(f64::from(constraints.wall_detour_m))
    } else {
        base
    }
}

fn ground_target(
    hf: &HeightField,
    constraints: &Constraints,
    desired: DVec3,
    velocity: DVec3,
) -> f64 {
    let radius = footprint_radius(hf, constraints);
    let mut floor = footprint(hf, desired.x, desired.y, radius);
    let horizontal = DVec2::new(velocity.x, velocity.y);
    let speed = horizontal.length();
    if speed > 1.0 {
        let direction = DVec2::new(velocity.x / speed, velocity.y / speed);
        let look = f64::from(constraints.wall_look_ahead_s.max(0.5));
        for step in 1..=4 {
            let distance = speed * look * f64::from(step) / 4.0;
            let x = desired.x + direction.x * distance;
            let y = desired.y + direction.y * distance;
            let ahead = footprint(hf, x, y, radius) + f64::from(constraints.ground_clearance_m);
            // Only anticipate a rise that would actually intersect the
            // authored altitude. Otherwise a high flyover gets lifted for no
            // reason by a hill that is still below the path.
            if ahead > desired.z {
                floor = floor.max(ahead);
            }
        }
    }
    floor += f64::from(constraints.ground_clearance_m);
    if constraints.reference_enabled {
        floor = floor
            .max(f64::from(constraints.reference_z_m) + f64::from(constraints.ground_clearance_m));
    }
    floor
}

fn hard_floor(hf: &HeightField, constraints: &Constraints, x: f64, y: f64) -> f64 {
    let radius = footprint_radius(hf, constraints);
    let mut floor = footprint(hf, x, y, radius) + f64::from(constraints.ground_clearance_m);
    if constraints.reference_enabled {
        floor = floor
            .max(f64::from(constraints.reference_z_m) + f64::from(constraints.ground_clearance_m));
    }
    floor
}

fn footprint_radius(hf: &HeightField, constraints: &Constraints) -> f64 {
    let (sx, sy) = hf.spacing();
    (sx.max(sy) * 1.1).max(f64::from(constraints.wall_margin_m))
}

fn footprint(hf: &HeightField, x: f64, y: f64, radius: f64) -> f64 {
    let mut max = terrain_at(hf, x, y);
    for (dx, dy) in [
        (1.0, 0.0),
        (-1.0, 0.0),
        (0.0, 1.0),
        (0.0, -1.0),
        (0.707, 0.707),
        (-0.707, 0.707),
        (0.707, -0.707),
        (-0.707, -0.707),
    ] {
        max = max.max(terrain_at(hf, x + dx * radius, y + dy * radius));
    }
    max
}

fn terrain_at(hf: &HeightField, x: f64, y: f64) -> f64 {
    let (ox, oy) = hf.origin();
    let (sx, sy) = hf.spacing();
    let gx = (x - ox) / sx;
    let gy = (y - oy) / sy;
    f64::from(hf.sample_bilinear(gx, gy))
}

fn smooth_floor(current: f64, target: f64, dt: f64, softness: f32) -> f64 {
    let soft = f64::from(softness.clamp(0.0, 1.0));
    let tau = if target > current {
        0.18 + soft * 0.35
    } else {
        0.9 + soft * 1.2
    };
    let alpha = 1.0 - (-dt / tau).exp();
    current + (target - current) * alpha
}

fn enforce_hard(
    hf: &HeightField,
    clearance: &ClearanceField,
    constraints: &Constraints,
    point: &mut DVec3,
) {
    if !constraints.enabled {
        return;
    }
    if constraints.ground_guard {
        let floor = hard_floor(hf, constraints, point.x, point.y);
        point.z = point.z.max(floor);
    }
    if constraints.wall_avoidance && constraints.wall_distance_m > 0.0 {
        let (distance, _, _, top) = clearance.sample(point.x, point.y);
        if distance.is_finite() && !distance.is_infinite() {
            let margin = f64::from(constraints.wall_distance_m + constraints.wall_margin_m);
            let vertical = f64::from(constraints.wall_vertical_margin_m);
            if f64::from(distance) < margin && point.z < f64::from(top) + vertical {
                point.z = f64::from(top) + vertical;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::RigSimulation;
    use vf_core::{AircraftPreset, PathKey, Seed};

    fn flat() -> HeightField {
        HeightField::filled(129, 129, 10.0, 0.0).unwrap()
    }

    fn cliff() -> HeightField {
        let mut heights = vec![0.0f32; 129 * 129];
        for y in 0..129 {
            for x in 64..129 {
                heights[y * 129 + x] = 200.0;
            }
        }
        HeightField::new(129, 129, 10.0, 10.0, heights).unwrap()
    }

    fn rig(preset: AircraftPreset) -> CameraRig {
        let mut rig = CameraRig::preset(preset.name()).unwrap();
        rig.eye.keys = vec![
            PathKey::new(0.0, [100.0, 640.0, 50.0]),
            PathKey::new(20.0, [1200.0, 640.0, 50.0]),
        ];
        rig.duration_s = 20.0;
        rig.target.mode = vf_core::TargetMode::Offset;
        rig.target.offset = [0.0, 300.0, 0.0];
        rig.exact_path = true;
        rig.constraints.enabled = true;
        rig.constraints.ground_guard = true;
        rig.constraints.ground_clearance_m = 20.0;
        rig.constraints.wall_avoidance = true;
        rig.constraints.wall_distance_m = 40.0;
        rig.constraints.wall_margin_m = 5.0;
        rig.constraints.wall_look_ahead_s = 4.0;
        rig
    }

    #[test]
    fn ground_guard_never_penetrates_flat_ground() {
        let hf = flat();
        let mut rig = rig(AircraftPreset::DroneFpv);
        rig.eye.keys[0].position[2] = -50.0;
        rig.eye.keys[1].position[2] = -50.0;
        let mut sim = RigSimulation::new(&rig, &hf, Seed::new(1)).unwrap();
        for index in 0..=100 {
            let t = 20.0 * index as f32 / 100.0;
            let sample = sim.sample_at(t);
            assert!(
                sample.eye.z >= 20.0,
                "t={t} eye z={} below clearance",
                sample.eye.z
            );
        }
    }

    #[test]
    fn wall_guard_does_not_cross_a_tall_cliff() {
        let hf = cliff();
        let rig = rig(AircraftPreset::DroneFpv);
        let mut sim = RigSimulation::new(&rig, &hf, Seed::new(1)).unwrap();
        let mut max_penetration = 0.0f64;
        for index in 0..=100 {
            let t = 20.0 * index as f32 / 100.0;
            let sample = sim.sample_at(t);
            if sample.eye.x > 640.0 && sample.eye.x < 1290.0 {
                let ground = if sample.eye.x >= 640.0 { 200.0 } else { 0.0 };
                max_penetration = max_penetration.max((sample.eye.z - ground).min(0.0).abs());
            }
        }
        assert!(
            max_penetration < 1.0,
            "camera entered cliff by {max_penetration} m"
        );
    }

    #[test]
    fn lateral_detour_goes_around_a_finite_wall_and_returns() {
        // Tall wall (1000 m) on x=600..800, y=600..680. An airship cannot
        // climb it in the available time, so the guard must go around the
        // short side (y) and return to the authored y=640 line.
        let mut heights = vec![0.0f32; 129 * 129];
        for y in 60..68 {
            for x in 60..80 {
                heights[y * 129 + x] = 1000.0;
            }
        }
        let hf = HeightField::new(129, 129, 10.0, 10.0, heights).unwrap();
        let mut rig = CameraRig::preset("airship").unwrap();
        rig.eye.keys = vec![
            PathKey::new(0.0, [100.0, 640.0, 100.0]),
            PathKey::new(40.0, [1200.0, 640.0, 100.0]),
        ];
        rig.duration_s = 40.0;
        rig.exact_path = true;
        rig.constraints.enabled = true;
        rig.constraints.ground_guard = false;
        rig.constraints.wall_avoidance = true;
        rig.constraints.wall_distance_m = 40.0;
        rig.constraints.wall_margin_m = 5.0;
        rig.constraints.wall_look_ahead_s = 4.0;
        rig.constraints.wall_vertical_margin_m = 5.0;
        let mut sim = RigSimulation::new(&rig, &hf, Seed::new(1)).unwrap();
        let mut max_y = 0.0f64;
        let mut min_clearance = f64::INFINITY;
        let mut y_after = 0.0f64;
        for index in 0..=400 {
            let t = 40.0 * index as f32 / 400.0;
            let sample = sim.sample_at(t);
            max_y = max_y.max((sample.eye.y - 640.0).abs());
            let (distance, _, _, _) = sim.clearance().sample(sample.eye.x, sample.eye.y);
            if sample.eye.x > 550.0 && sample.eye.x < 850.0 {
                min_clearance = min_clearance.min(f64::from(distance));
            }
            if sample.eye.x > 850.0 {
                y_after = (sample.eye.y - 640.0).abs();
            }
        }
        assert!(max_y > 40.0, "no lateral detour, max y offset {max_y}");
        assert!(y_after < 20.0, "did not return to path, y offset {y_after}");
        assert!(min_clearance > 5.0, "wall clearance {min_clearance}");
    }

    #[test]
    fn low_wall_is_climbed_instead_of_detoured() {
        let mut heights = vec![0.0f32; 129 * 129];
        for y in 60..68 {
            for x in 60..80 {
                heights[y * 129 + x] = 100.0;
            }
        }
        let hf = HeightField::new(129, 129, 10.0, 10.0, heights).unwrap();
        let mut rig = rig(AircraftPreset::DroneFpv);
        rig.eye.keys = vec![
            PathKey::new(0.0, [100.0, 640.0, 50.0]),
            PathKey::new(20.0, [1200.0, 640.0, 50.0]),
        ];
        rig.duration_s = 20.0;
        rig.constraints.ground_guard = false;
        rig.constraints.wall_look_ahead_s = 3.0;
        let mut sim = RigSimulation::new(&rig, &hf, Seed::new(1)).unwrap();
        let mut climbed = false;
        for index in 0..=100 {
            let t = 20.0 * index as f32 / 100.0;
            let sample = sim.sample_at(t);
            if sample.eye.x > 580.0 && sample.eye.x < 820.0 {
                eprintln!(
                    "x={:.0} z={:.1} flags={:?}",
                    sample.eye.x, sample.eye.z, sample.safety
                );
                climbed |= sample.eye.z >= 105.0;
            }
        }
        assert!(climbed, "camera never climbed above the low wall");
    }

    #[test]
    fn safety_is_inactive_when_disabled() {
        let hf = flat();
        let mut rig = rig(AircraftPreset::DroneFpv);
        rig.eye.keys[0].position[2] = -50.0;
        rig.eye.keys[1].position[2] = -50.0;
        rig.constraints.enabled = false;
        let mut sim = RigSimulation::new(&rig, &hf, Seed::new(1)).unwrap();
        let sample = sim.sample_at(10.0);
        assert!(sample.eye.z < -10.0);
    }
}
