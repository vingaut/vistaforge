//! Camera framing and scene helpers shared by the CLI, renderer and UI.

use glam::DVec3;
use vf_core::{Camera, HeightField};

/// Standard CAD viewpoints around the terrain.
///
/// The renderer keeps a perspective camera, so these are framing directions,
/// not orthographic projections: they put the eye exactly on the axis and
/// look at the terrain centre.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewPreset {
    /// Three-quarter view used as the default/home position.
    Perspective,
    /// Looking north from the south (yaw 0).
    Front,
    /// Looking south from the north (yaw 180).
    Back,
    /// Looking east from the west (yaw -90).
    Left,
    /// Looking west from the east (yaw 90).
    Right,
    /// Looking straight down (pitch 90).
    Top,
    /// Looking straight up (pitch -90).
    Bottom,
}

impl ViewPreset {
    pub const ALL: [Self; 7] = [
        Self::Perspective,
        Self::Front,
        Self::Right,
        Self::Back,
        Self::Left,
        Self::Top,
        Self::Bottom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Perspective => "Perspective",
            Self::Front => "Face",
            Self::Back => "Arrière",
            Self::Left => "Gauche",
            Self::Right => "Droite",
            Self::Top => "Dessus",
            Self::Bottom => "Dessous",
        }
    }

    /// Orbit yaw/pitch in degrees. `None` means "use the terrain auto-frame".
    pub fn yaw_pitch(self) -> Option<(f32, f32)> {
        match self {
            Self::Perspective => None,
            Self::Front => Some((0.0, 0.0)),
            Self::Back => Some((180.0, 0.0)),
            Self::Left => Some((-90.0, 0.0)),
            Self::Right => Some((90.0, 0.0)),
            Self::Top => Some((0.0, 90.0)),
            Self::Bottom => Some((0.0, -90.0)),
        }
    }

    /// View from the direction of `normal` (one of ±X, ±Y, ±Z). Used by the
    /// axis gizmo.
    pub fn from_normal(normal: DVec3) -> Option<Self> {
        let n = normal.normalize_or_zero();
        [
            (DVec3::X, Self::Right),
            (DVec3::NEG_X, Self::Left),
            (DVec3::Y, Self::Back),
            (DVec3::NEG_Y, Self::Front),
            (DVec3::Z, Self::Top),
            (DVec3::NEG_Z, Self::Bottom),
        ]
        .into_iter()
        .find(|(axis, _)| (*axis - n).length() < 0.1)
        .map(|(_, preset)| preset)
    }
}

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
    auto_frame_with_fov(hf, aspect, 48.0)
}

/// Same as [`auto_frame`] but with an explicit vertical field of view.
pub fn auto_frame_with_fov(hf: &HeightField, aspect: f64, fov_y_deg: f32) -> Camera {
    let bounds = hf.bounds();
    let center = terrain_center(hf);
    let extent = terrain_extent(hf);
    let fov_y = fov_y_deg.clamp(1.0, 170.0);
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

/// Up vector used by [`orbit`].
///
/// Looking straight down with `Z` as the up hint is degenerate. Near the poles
/// we switch to a horizontal north/south up, which keeps the top view map-like
/// (north up, east right) instead of spinning around the singularity.
pub fn orbit_up(pitch_deg: f32) -> DVec3 {
    if pitch_deg >= 89.5 {
        DVec3::Y
    } else if pitch_deg <= -89.5 {
        DVec3::NEG_Y
    } else {
        DVec3::Z
    }
}

/// Orbit camera around a target. `yaw_deg` 0 places the camera due south of
/// the target and increases toward east; `pitch_deg` is the elevation above
/// the horizon.
pub fn orbit(target: DVec3, distance: f64, yaw_deg: f32, pitch_deg: f32, fov_y_deg: f32) -> Camera {
    let yaw = f64::from(yaw_deg).to_radians();
    let pitch_deg = pitch_deg.clamp(-90.0, 90.0);
    let pitch = f64::from(pitch_deg).to_radians();
    let offset = DVec3::new(
        yaw.sin() * pitch.cos(),
        -yaw.cos() * pitch.cos(),
        pitch.sin(),
    ) * distance.max(0.1);
    let position = target + offset;
    Camera {
        position,
        target,
        up: orbit_up(pitch_deg),
        fov_y_deg: fov_y_deg.clamp(1.0, 170.0),
        near: (distance * 0.001).max(0.05) as f32,
        far: (distance * 100.0 + 10_000.0) as f32,
        focal_length_mm: None,
        sensor_mm: 36.0,
    }
}

/// Distance at which the terrain's bounding sphere fits the frustum.
pub fn fit_distance(hf: &HeightField, aspect: f64, fov_y_deg: f32) -> f64 {
    let extent = terrain_extent(hf);
    let fov = f64::from(fov_y_deg.clamp(1.0, 170.0)).to_radians();
    let vertical = (fov * 0.5).tan();
    let horizontal = vertical * aspect.max(0.2);
    let fit = extent / (2.0 * vertical.min(horizontal).max(0.05));
    (fit * 1.1).max(extent * 0.6)
}

/// Frame the terrain from one of the standard CAD viewpoints.
///
/// [`ViewPreset::Perspective`] is the default three-quarter home view. The
/// other presets orbit around the terrain centre at a distance that fits the
/// whole heightfield in view.
pub fn framed_view(hf: &HeightField, aspect: f64, preset: ViewPreset, fov_y_deg: f32) -> Camera {
    let Some((yaw, pitch)) = preset.yaw_pitch() else {
        return auto_frame_with_fov(hf, aspect, fov_y_deg);
    };
    let center = terrain_center(hf);
    let distance = fit_distance(hf, aspect, fov_y_deg);
    orbit(center, distance, yaw, pitch, fov_y_deg)
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

    #[test]
    fn top_view_is_not_degenerate_and_north_is_up() {
        let hf = terrain();
        let cam = framed_view(&hf, 16.0 / 9.0, ViewPreset::Top, 48.0);
        cam.validate().unwrap();
        assert!((cam.up - DVec3::Y).length() < 1e-9);
        assert!(cam.right().dot(DVec3::X) > 0.99);
        assert!(cam.true_up().dot(DVec3::Y) > 0.99);
        assert!((cam.position.z - hf.bounds().max[2]).abs() > 0.0);
    }

    #[test]
    fn side_views_look_along_the_axes() {
        let hf = terrain();
        for (preset, forward) in [
            (ViewPreset::Front, DVec3::Y),
            (ViewPreset::Back, DVec3::NEG_Y),
            (ViewPreset::Right, DVec3::NEG_X),
            (ViewPreset::Left, DVec3::X),
        ] {
            let cam = framed_view(&hf, 16.0 / 9.0, preset, 48.0);
            cam.validate().unwrap();
            assert!(
                cam.forward().dot(forward) > 0.999,
                "{preset:?} forward {:?}",
                cam.forward()
            );
            assert!((cam.up - DVec3::Z).length() < 1e-9);
        }
    }

    #[test]
    fn presets_round_trip_through_orbit_angles() {
        let hf = terrain();
        for preset in ViewPreset::ALL {
            let Some((yaw, pitch)) = preset.yaw_pitch() else {
                continue;
            };
            let camera = framed_view(&hf, 16.0 / 9.0, preset, 48.0);
            let center = terrain_center(&hf);
            let delta = camera.position - center;
            let distance = delta.length();
            let round_trip = orbit(center, distance, yaw, pitch, 48.0);
            assert!(
                (round_trip.position - camera.position).length() < 1e-6,
                "{preset:?} position drift"
            );
            assert!(
                (round_trip.up - camera.up).length() < 1e-6,
                "{preset:?} up drift"
            );
        }
    }

    #[test]
    fn normal_mapping_matches_view_cube_faces() {
        assert_eq!(ViewPreset::from_normal(DVec3::Z), Some(ViewPreset::Top));
        assert_eq!(
            ViewPreset::from_normal(DVec3::NEG_Z),
            Some(ViewPreset::Bottom)
        );
        assert_eq!(
            ViewPreset::from_normal(DVec3::NEG_Y),
            Some(ViewPreset::Front)
        );
        assert_eq!(ViewPreset::from_normal(DVec3::X), Some(ViewPreset::Right));
        assert_eq!(ViewPreset::from_normal(DVec3::new(1.0, 1.0, 0.0)), None);
    }
}
