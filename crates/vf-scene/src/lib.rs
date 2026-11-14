//! Camera framing and scene helpers shared by the CLI, renderer and UI.

use glam::DVec3;
use vf_core::{Camera, HeightField};

/// World-space centre of a heightfield's bounding box.
pub fn terrain_center(hf: &HeightField) -> DVec3 {
    let b = hf.bounds();
    let c = b.center();
    DVec3::new(c[0], c[1], c[2])
}

/// Horizontal extent and relief used to choose a viewing distance.
pub fn terrain_extent(hf: &HeightField) -> f64 {
    let b = hf.bounds();
    let s = b.size();
    s[0].max(s[1]).max(s[2] * 1.2).max(1.0)
}

/// Frame the whole terrain from the south-east, slightly above the horizon.
pub fn auto_frame(hf: &HeightField, aspect: f64) -> Camera {
    let bounds = hf.bounds();
    let center = terrain_center(hf);
    let extent = terrain_extent(hf);
    let fov_y = 48.0f32;
    let fov = f64::from(fov_y).to_radians();
    let vertical = (fov * 0.5).tan();
    let horizontal = vertical * aspect.max(0.2);
    let fit = extent / (2.0 * vertical.min(horizontal).max(0.05));
    let distance = (fit * 1.05).max(extent * 0.85);

    let offset = DVec3::new(0.45, -0.82, 0.28).normalize();
    let mut position = center + offset * distance;
    let target = DVec3::new(
        center.x,
        center.y,
        bounds.min[2] + (bounds.max[2] - bounds.min[2]) * 0.38,
    );

    // Keep the eye above the terrain under it.
    let (ox, oy) = hf.origin();
    let (sx, sy) = hf.spacing();
    let gx = ((position.x - ox) / sx).clamp(0.0, (hf.width() - 1) as f64);
    let gy = ((position.y - oy) / sy).clamp(0.0, (hf.height() - 1) as f64);
    let ground = f64::from(hf.sample_bilinear(gx, gy));
    position.z = position.z.max(ground + extent * 0.06);

    Camera {
        position,
        target,
        up: DVec3::Z,
        fov_y_deg: fov_y,
        near: (extent * 0.0005).max(0.05) as f32,
        far: ((distance + extent) * 30.0) as f32,
        focal_length_mm: None,
        sensor_mm: 36.0,
    }
}

/// Orbit camera around a target. `yaw_deg` 0 places the camera due south of
/// the target and increases toward east; `pitch_deg` is the elevation above
/// the horizon.
pub fn orbit(target: DVec3, distance: f64, yaw_deg: f32, pitch_deg: f32, fov_y_deg: f32) -> Camera {
    let yaw = f64::from(yaw_deg).to_radians();
    let pitch = f64::from(pitch_deg.clamp(-85.0, 85.0)).to_radians();
    let offset = DVec3::new(
        yaw.sin() * pitch.cos(),
        -yaw.cos() * pitch.cos(),
        pitch.sin(),
    ) * distance.max(0.1);
    let position = target + offset;
    Camera {
        position,
        target,
        up: DVec3::Z,
        fov_y_deg: fov_y_deg.clamp(1.0, 170.0),
        near: (distance * 0.001).max(0.05) as f32,
        far: (distance * 100.0 + 10_000.0) as f32,
        focal_length_mm: None,
        sensor_mm: 36.0,
    }
}

/// Camera looking from `eye` at `target`.
pub fn look_at(eye: DVec3, target: DVec3, fov_y_deg: f32) -> Camera {
    let distance = (target - eye).length().max(0.1);
    Camera {
        position: eye,
        target,
        up: DVec3::Z,
        fov_y_deg,
        near: (distance * 0.001).max(0.05) as f32,
        far: (distance * 100.0 + 10_000.0) as f32,
        focal_length_mm: None,
        sensor_mm: 36.0,
    }
}

/// Elevate a camera that ended up below the terrain at its X/Y position.
pub fn keep_above_terrain(camera: &mut Camera, hf: &HeightField) {
    let (ox, oy) = hf.origin();
    let (sx, sy) = hf.spacing();
    let gx = ((camera.position.x - ox) / sx).clamp(0.0, (hf.width() - 1) as f64);
    let gy = ((camera.position.y - oy) / sy).clamp(0.0, (hf.height() - 1) as f64);
    let ground = f64::from(hf.sample_bilinear(gx, gy));
    if camera.position.z < ground + 1.0 {
        camera.position.z = ground + 1.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terrain() -> HeightField {
        let n = 65;
        let heights = (0..n * n).map(|i| ((i % n) as f32) * 20.0).collect();
        HeightField::new(n, n, 30.0, 30.0, heights).unwrap()
    }

    #[test]
    fn auto_frame_sees_the_terrain() {
        let hf = terrain();
        let cam = auto_frame(&hf, 16.0 / 9.0);
        cam.validate().unwrap();
        assert!(cam.position.z > 0.0);
        let center = terrain_center(&hf);
        assert!((cam.target - center).length() < hf.bounds().extent());
    }

    #[test]
    fn orbit_keeps_distance() {
        let target = DVec3::ZERO;
        let cam = orbit(target, 1000.0, 45.0, 30.0, 50.0);
        assert!(((cam.position - target).length() - 1000.0).abs() < 1e-2);
        cam.validate().unwrap();
    }
}
