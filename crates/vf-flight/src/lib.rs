//! Deterministic aerial camera flight.
//!
//! The camera is not a keyframe interpolation. A path is generated over the
//! terrain, then a light flight-dynamics model produces the attitude:
//!
//! * the path sets the velocity direction;
//! * the bank angle comes from the actual centripetal acceleration
//!   (`atan(v²·κ/g)`), which is what makes a turn look flown rather than slid;
//! * attitude follows with a first-order lag, so fast presets feel twitchy and
//!   heavy presets feel like a helicopter;
//! * seeded fractal shake and gusts add the last few percent of realism.
//!
//! Everything is a pure function of the settings, the terrain and the seed:
//! same inputs, same frames, on every machine.

use glam::DVec3;
pub mod clearance;
pub mod rig;
pub mod safety;

use vf_core::{Camera, Error, FlightPattern, FlightSettings, HeightField, Result, Seed};
use vf_procgen::Noise;

/// One simulated frame.
#[derive(Debug, Clone, PartialEq)]
pub struct FlightSample {
    pub time_s: f64,
    pub camera: Camera,
    pub speed_mps: f64,
    pub bank_deg: f64,
    pub altitude_agl_m: f64,
}

const G: f64 = 9.81;
/// Dense polyline step, metres.
const PATH_STEP: f64 = 4.0;

/// Simulate the whole clip.
pub fn simulate(
    settings: &FlightSettings,
    hf: &HeightField,
    seed: Seed,
    fov_y_deg: f32,
) -> Result<Vec<FlightSample>> {
    settings.validate().map_err(Error::Invalid)?;
    let frames = settings.frame_count();
    let fps = f64::from(settings.fps.max(1));
    let dt = 1.0 / fps;
    let path = Path::build(settings, hf, seed);
    if path.points.len() < 2 {
        return Err(Error::Invalid("flight path is degenerate".into()));
    }

    let speed = f64::from(settings.speed_mps);
    let tau = 0.03 + f64::from(settings.smoothness) * 0.55;
    let alpha = 1.0 - (-dt / tau).exp();
    let shake_noise = Noise::from_u64(seed.derive("flight-shake").0);
    let gust_noise = Noise::from_u64(seed.derive("flight-gust").0);
    let shake = f64::from(settings.shake_deg);
    let turbulence = f64::from(settings.turbulence);

    let bounds = hf.bounds();
    let aim = DVec3::new(
        (bounds.min[0] + bounds.max[0]) * 0.5,
        (bounds.min[1] + bounds.max[1]) * 0.5,
        (bounds.min[2] + bounds.max[2]) * 0.5,
    );
    let mut yaw = path.tangent(0.0).y.atan2(path.tangent(0.0).x);
    let mut pitch = 0.0f64;
    let mut roll = 0.0f64;
    let mut samples = Vec::with_capacity(frames);

    for frame in 0..frames {
        let t = frame as f64 * dt;
        // Ease the first two seconds so the clip does not start at full speed.
        let tau_speed = 1.6;
        let distance = if matches!(
            settings.pattern,
            FlightPattern::Orbit | FlightPattern::Racetrack
        ) {
            speed * t
        } else {
            speed * (t - tau_speed * (1.0 - (-t / tau_speed).exp())).max(0.0)
        };
        let d = path.wrap(distance);
        let position = path.position(d);
        let tangent = path.tangent(d);
        let turn = path.turn(d);
        let curvature = path.curvature(d);

        let (target_yaw, target_pitch) = if settings.look_at_center {
            let to = aim - position;
            let horizontal = (to.x * to.x + to.y * to.y).sqrt().max(1.0);
            (
                to.x.atan2(to.y),
                (to.z / horizontal).atan().clamp(-0.6, 0.6),
            )
        } else {
            (
                tangent.x.atan2(tangent.y),
                tangent.z.clamp(-1.0, 1.0).asin() * 0.45 - speed * 0.004,
            )
        };
        let bank = (speed * speed * curvature / G).atan() * f64::from(settings.bank_gain);
        let bank = bank.min(f64::from(settings.max_bank_deg).to_radians());
        let target_roll = if settings.gimbal {
            0.0
        } else {
            bank * turn.z.signum()
        };

        yaw += wrap_angle(target_yaw - yaw) * alpha;
        pitch += (target_pitch - pitch) * alpha;
        roll += (target_roll - roll) * alpha;

        // Seeded shake: three decorrelated bands, stronger at speed.
        let speed_factor = 0.55 + speed / 45.0;
        let shake_amp = shake.to_radians() * speed_factor;
        let sh_yaw = shake_noise.fbm2(t as f32 * 0.7, 11.3, 3, 2.0, 0.5) as f64 * shake_amp;
        let sh_pitch = shake_noise.fbm2(t as f32 * 0.9, 27.1, 3, 2.0, 0.5) as f64 * shake_amp;
        let sh_roll = shake_noise.fbm2(t as f32 * 0.6, 43.7, 3, 2.0, 0.5) as f64 * shake_amp * 1.4;

        // Gusts drift the airframe, not the camera aim.
        let gust = DVec3::new(
            gust_noise.fbm2(t as f32 * 0.25, 3.1, 2, 2.0, 0.5) as f64,
            gust_noise.fbm2(t as f32 * 0.22, 8.7, 2, 2.0, 0.5) as f64,
            gust_noise.fbm2(t as f32 * 0.18, 15.3, 2, 2.0, 0.5) as f64 * 0.35,
        ) * (turbulence * 2.5);

        let camera_yaw = yaw + sh_yaw;
        let camera_pitch = (pitch
            + f64::from(settings.uptilt_deg).to_radians() * 0.25
            + f64::from(settings.pitch_bias_deg).to_radians()
            + sh_pitch)
            .clamp(-1.45, 1.45);
        let camera_roll = if settings.gimbal {
            sh_roll * 0.25
        } else {
            roll + sh_roll
        };

        let forward = DVec3::new(
            camera_yaw.sin() * camera_pitch.cos(),
            camera_yaw.cos() * camera_pitch.cos(),
            camera_pitch.sin(),
        )
        .normalize_or_zero();
        let up = rolled_up(forward, camera_roll);

        let mut eye = position + gust;
        let ground = ground_at(hf, eye.x, eye.y);
        let clearance = clearance_at(hf, eye.x, eye.y, 40.0) + 4.0;
        if eye.z < clearance {
            eye.z = clearance;
        }
        let target = eye + forward * 100.0;
        let camera = Camera {
            position: eye,
            target,
            up,
            fov_y_deg,
            near: 0.5,
            far: 200_000.0,
            focal_length_mm: None,
            sensor_mm: 36.0,
        };
        samples.push(FlightSample {
            time_s: t,
            camera,
            speed_mps: speed,
            bank_deg: camera_roll.to_degrees(),
            altitude_agl_m: (eye.z - ground).max(0.0),
        });
    }
    Ok(samples)
}

/// One simulated sample at a given clip time.
pub fn sample_at(
    settings: &FlightSettings,
    hf: &HeightField,
    seed: Seed,
    fov_y_deg: f32,
    time_s: f64,
) -> Result<FlightSample> {
    let samples = simulate(settings, hf, seed, fov_y_deg)?;
    let fps = f64::from(settings.fps.max(1));
    let index = ((time_s * fps).round() as usize).min(samples.len().saturating_sub(1));
    Ok(samples[index].clone())
}

/// One camera at a given clip time, for stills and previews.
pub fn camera_at(
    settings: &FlightSettings,
    hf: &HeightField,
    seed: Seed,
    fov_y_deg: f32,
    time_s: f64,
) -> Result<Camera> {
    Ok(sample_at(settings, hf, seed, fov_y_deg, time_s)?.camera)
}

fn wrap_angle(a: f64) -> f64 {
    let mut a = a;
    while a > std::f64::consts::PI {
        a -= std::f64::consts::TAU;
    }
    while a < -std::f64::consts::PI {
        a += std::f64::consts::TAU;
    }
    a
}

fn rolled_up(forward: DVec3, roll: f64) -> DVec3 {
    let right = forward.cross(DVec3::Z).normalize_or_zero();
    if right.length_squared() < 1e-9 {
        return DVec3::Y;
    }
    let up0 = right.cross(forward).normalize_or_zero();
    (up0 * roll.cos() + right * roll.sin()).normalize_or_zero()
}

/// Highest ground within `radius`, so the aircraft does not clip a ridge that
/// sits between two path samples.
fn clearance_at(hf: &HeightField, x: f64, y: f64, radius: f64) -> f64 {
    let mut highest = ground_at(hf, x, y);
    for i in 0..8 {
        let a = i as f64 / 8.0 * std::f64::consts::TAU;
        for r in [radius * 0.5, radius] {
            highest = highest.max(ground_at(hf, x + r * a.cos(), y + r * a.sin()));
        }
    }
    highest
}

fn ground_at(hf: &HeightField, x: f64, y: f64) -> f64 {
    let (ox, oy) = hf.origin();
    let (sx, sy) = hf.spacing();
    let gx = ((x - ox) / sx).clamp(0.0, (hf.width() - 1) as f64);
    let gy = ((y - oy) / sy).clamp(0.0, (hf.height() - 1) as f64);
    f64::from(hf.sample_bilinear(gx, gy))
}

struct Path {
    points: Vec<DVec3>,
    cumulative: Vec<f64>,
    length: f64,
    closed: bool,
}

impl Path {
    fn build(settings: &FlightSettings, hf: &HeightField, seed: Seed) -> Self {
        let control = control_points(settings, hf);
        let closed = matches!(
            settings.pattern,
            FlightPattern::Orbit | FlightPattern::Racetrack
        );
        let mut dense = catmull_rom(&control, closed);
        // Smooth the altitude a little: terrain bumps must not become camera
        // bumps, the altitude hold of a real aircraft filters them out.
        let agl = f64::from(settings.altitude_agl_m);
        for _ in 0..3 {
            smooth_altitude(&mut dense, hf, agl);
        }
        let _ = seed;
        let mut cumulative = Vec::with_capacity(dense.len());
        let mut total = 0.0;
        for (i, p) in dense.iter().enumerate() {
            if i > 0 {
                total += (*p - dense[i - 1]).length();
            }
            cumulative.push(total);
        }
        Self {
            points: dense,
            cumulative,
            length: total.max(1e-6),
            closed,
        }
    }

    fn wrap(&self, distance: f64) -> f64 {
        if self.closed {
            distance.rem_euclid(self.length)
        } else {
            distance.clamp(0.0, self.length)
        }
    }

    fn position(&self, distance: f64) -> DVec3 {
        let d = self.wrap(distance);
        let index = match self
            .cumulative
            .binary_search_by(|probe| probe.partial_cmp(&d).unwrap_or(std::cmp::Ordering::Equal))
        {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let i = index.min(self.points.len() - 1);
        let j = (i + 1).min(self.points.len() - 1);
        let span = self.cumulative[j] - self.cumulative[i];
        let t = if span > 1e-9 {
            ((d - self.cumulative[i]) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.points[i].lerp(self.points[j], t)
    }

    fn tangent(&self, distance: f64) -> DVec3 {
        let eps = 2.0;
        let a = self.position(distance - eps);
        let b = self.position(distance + eps);
        (b - a).normalize_or_zero()
    }

    fn curvature(&self, distance: f64) -> f64 {
        // A wide, averaged window rejects the faceting of the polyline; a
        // tighter one makes the bank jitter on every vertex.
        let eps = 30.0;
        let mut total = 0.0;
        for step in 0..4 {
            let offset = eps * step as f64 / 3.0;
            let t0 = self.tangent(distance - offset - 10.0);
            let t1 = self.tangent(distance + offset + 10.0);
            total += (t1 - t0).length() / 20.0;
        }
        (total / 4.0).min(1.0 / 40.0)
    }

    fn turn(&self, distance: f64) -> DVec3 {
        self.tangent(distance - 4.0)
            .cross(self.tangent(distance + 4.0))
    }
}

fn control_points(settings: &FlightSettings, hf: &HeightField) -> Vec<DVec3> {
    let bounds = hf.bounds();
    let center = DVec3::new(
        (bounds.min[0] + bounds.max[0]) * 0.5,
        (bounds.min[1] + bounds.max[1]) * 0.5,
        0.0,
    );
    let extent = (bounds.max[0] - bounds.min[0])
        .max(bounds.max[1] - bounds.min[1])
        .max(1.0);
    let agl = f64::from(settings.altitude_agl_m);
    let mut points = Vec::new();
    match settings.pattern {
        FlightPattern::Orbit => {
            let radius = extent * 0.40;
            for i in 0..10 {
                let a = i as f64 / 10.0 * std::f64::consts::TAU;
                let x = center.x + radius * a.cos();
                let y = center.y + radius * a.sin();
                points.push(DVec3::new(x, y, ground_at(hf, x, y) + agl));
            }
        }
        FlightPattern::Racetrack => {
            let rx = extent * 0.44;
            let ry = extent * 0.24;
            for i in 0..12 {
                let a = i as f64 / 12.0 * std::f64::consts::TAU;
                let x = center.x + rx * a.cos();
                let y = center.y + ry * a.sin();
                points.push(DVec3::new(x, y, ground_at(hf, x, y) + agl));
            }
        }
        FlightPattern::RidgeRun => {
            let half = extent * 0.42;
            for i in 0..7 {
                let u = i as f64 / 6.0 - 0.5;
                let x = center.x + extent * 0.08 + (u * 6.0).sin() * extent * 0.10;
                let y = center.y + u * 2.0 * half;
                points.push(DVec3::new(x, y, ground_at(hf, x, y) + agl));
            }
        }
        FlightPattern::Dive => {
            let start = DVec3::new(
                center.x + extent * 0.30,
                center.y - extent * 0.42,
                ground_at(hf, center.x + extent * 0.30, center.y - extent * 0.42)
                    + extent * 0.18
                    + agl,
            );
            let mid = DVec3::new(
                center.x + extent * 0.10,
                center.y - extent * 0.10,
                ground_at(hf, center.x + extent * 0.10, center.y - extent * 0.10) + agl * 3.0,
            );
            let low = DVec3::new(
                center.x,
                center.y + extent * 0.06,
                ground_at(hf, center.x, center.y + extent * 0.06) + agl,
            );
            let out = DVec3::new(
                center.x - extent * 0.25,
                center.y + extent * 0.32,
                ground_at(hf, center.x - extent * 0.25, center.y + extent * 0.32) + agl * 4.0,
            );
            points.extend([start, mid, low, out]);
        }
    }
    points
}

fn smooth_altitude(points: &mut [DVec3], hf: &HeightField, agl: f64) {
    if points.len() < 3 {
        return;
    }
    let mut z = points.iter().map(|p| p.z).collect::<Vec<_>>();
    for i in 0..points.len() {
        let a = z[i.saturating_sub(1)];
        let b = z[i];
        let c = z[(i + 1).min(z.len() - 1)];
        z[i] = (a + 2.0 * b + c) * 0.25;
    }
    for (point, value) in points.iter_mut().zip(z) {
        let ground = clearance_at(hf, point.x, point.y, 40.0) + agl;
        point.z = value.max(ground);
    }
}

fn catmull_rom(control: &[DVec3], closed: bool) -> Vec<DVec3> {
    let n = control.len();
    if n < 2 {
        return control.to_vec();
    }
    let get = |i: isize| -> DVec3 {
        if closed {
            control[((i % n as isize + n as isize) % n as isize) as usize]
        } else {
            control[(i.clamp(0, n as isize - 1)) as usize]
        }
    };
    let segments = if closed { n } else { n - 1 };
    let mut out = Vec::new();
    for i in 0..segments {
        let p0 = get(i as isize - 1);
        let p1 = get(i as isize);
        let p2 = get(i as isize + 1);
        let p3 = get(i as isize + 2);
        let length = (p2 - p1).length();
        let steps = ((length / PATH_STEP).ceil() as usize).max(1);
        for step in 0..steps {
            let t = step as f64 / steps as f64;
            out.push(catmull(p0, p1, p2, p3, t));
        }
    }
    if !closed {
        out.push(*control.last().expect("non-empty"));
    } else {
        out.push(out[0]);
    }
    out
}

fn catmull(p0: DVec3, p1: DVec3, p2: DVec3, p3: DVec3, t: f64) -> DVec3 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::FlightSettings;

    fn terrain(n: usize) -> HeightField {
        let mut heights = vec![0.0f32; n * n];
        for y in 0..n {
            for x in 0..n {
                heights[y * n + x] = 400.0 + 300.0 * (x as f32 / n as f32).sin();
            }
        }
        HeightField::new(n, n, 30.0, 30.0, heights).unwrap()
    }

    #[test]
    fn flight_is_deterministic() {
        let hf = terrain(65);
        let settings = FlightSettings::preset("cinelifter").unwrap();
        let a = simulate(&settings, &hf, Seed::new(3), 78.0).unwrap();
        let b = simulate(&settings, &hf, Seed::new(3), 78.0).unwrap();
        assert_eq!(a.len(), settings.frame_count());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.camera.position, y.camera.position);
            assert_eq!(x.camera.target, y.camera.target);
        }
    }

    #[test]
    fn camera_stays_above_terrain_and_is_valid() {
        let hf = terrain(65);
        for name in vf_core::FLIGHT_PRESETS {
            let settings = FlightSettings::preset(name).unwrap();
            let samples = simulate(&settings, &hf, Seed::new(9), 70.0).unwrap();
            assert!(!samples.is_empty());
            for s in &samples {
                s.camera.validate().unwrap();
                let g = ground_at(&hf, s.camera.position.x, s.camera.position.y);
                assert!(s.camera.position.z > g + 0.5, "{name} clipped the ground");
                assert!(s.altitude_agl_m > 0.0);
            }
        }
    }

    #[test]
    fn turns_bank_the_camera() {
        let hf = terrain(65);
        let settings = FlightSettings::preset("racing").unwrap();
        let samples = simulate(&settings, &hf, Seed::new(1), 75.0).unwrap();
        let max_bank = samples
            .iter()
            .map(|s| s.bank_deg.abs())
            .fold(0.0f64, f64::max);
        assert!(max_bank > 5.0, "no visible bank: {max_bank}");
    }

    #[test]
    fn camera_at_matches_first_frame() {
        let hf = terrain(33);
        let settings = FlightSettings::preset("cinematic").unwrap();
        let all = simulate(&settings, &hf, Seed::new(2), 60.0).unwrap();
        let one = camera_at(&settings, &hf, Seed::new(2), 60.0, 0.0).unwrap();
        assert_eq!(one.position, all[0].camera.position);
    }
}
