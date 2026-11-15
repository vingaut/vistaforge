//! Shared helpers for the animation workspace: tabs, path/profile sampling,
//! screen projection and terrain picking.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use glam::{DVec3, DVec4};
use vf_core::{AircraftPreset, Camera, CameraRig, HeightField, MotionProfile, Path3D, Seed};
use vf_flight::rig::RigSimulation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimTab {
    Path,
    Timing,
    Profile,
    Preview,
}

impl AnimTab {
    pub const ALL: [Self; 4] = [Self::Path, Self::Timing, Self::Profile, Self::Preview];

    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|tab| {
            tab.short().to_ascii_lowercase().contains(&normalized)
                || format!("{tab:?}").to_ascii_lowercase() == normalized
        })
    }

    pub fn short(self) -> &'static str {
        match self {
            Self::Path => "1 · Tracé",
            Self::Timing => "2 · Réglages",
            Self::Profile => "3 · Profil",
            Self::Preview => "4 · Lecture",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimViewMode {
    /// Orbit/edit camera: the path and the gizmo are visible.
    Design,
    /// Camera sampled from the animation: clean shot, overlays optional.
    Camera,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimSelection {
    EyeKey(usize),
    TargetKey(usize),
    FixedTarget,
}

pub const PRESET_COLORS: [Color32; 6] = [
    Color32::from_rgb(255, 138, 76),
    Color32::from_rgb(120, 205, 255),
    Color32::from_rgb(134, 224, 154),
    Color32::from_rgb(210, 165, 255),
    Color32::from_rgb(255, 92, 120),
    Color32::from_rgb(200, 205, 214),
];

pub fn preset_color(preset: AircraftPreset) -> Color32 {
    let index = AircraftPreset::ALL
        .iter()
        .position(|candidate| *candidate == preset)
        .unwrap_or(0);
    PRESET_COLORS[index % PRESET_COLORS.len()]
}

pub fn preset_character(preset: AircraftPreset) -> &'static str {
    match preset {
        AircraftPreset::DroneFpv => {
            "Vif et nerveux : accélérations franches, virages serrés, forte inclinaison."
        }
        AircraftPreset::Bird => {
            "Souple et flottant : réactions moyennes, turbulences et micro-secousses."
        }
        AircraftPreset::Helicopter => {
            "Posé et lourd : accélérations douces, faible roulis, nacelle très stable."
        }
        AircraftPreset::Airship => "Très lent et inertiel : grandes courbes, roulis presque nul.",
        AircraftPreset::FighterJet => {
            "Ultra rapide : réponses immédiates, virages violents, forte énergie."
        }
        AircraftPreset::Manual => {
            "Aucune dynamique : la caméra suit exactement le tracé, sans inertie."
        }
    }
}

/// Apply an aircraft preset to an existing rig without touching the authored
/// path, duration, target or seed.
pub fn apply_aircraft_preset(rig: &mut CameraRig, preset: AircraftPreset) {
    rig.body = MotionProfile::preset(preset);
    rig.name = format!("Rig {}", preset.label());
    match preset {
        AircraftPreset::DroneFpv => {
            rig.optics.fov_y_deg = 78.0;
            rig.optics.fstop = 2.8;
            rig.constraints.ground_clearance_m = 18.0;
            rig.constraints.enabled = true;
        }
        AircraftPreset::Bird => {
            rig.optics.fov_y_deg = 42.0;
            rig.optics.fstop = 5.6;
            rig.constraints.ground_clearance_m = 35.0;
            rig.constraints.enabled = true;
        }
        AircraftPreset::Helicopter => {
            rig.optics.fov_y_deg = 35.0;
            rig.optics.fstop = 8.0;
            rig.constraints.ground_clearance_m = 70.0;
            rig.constraints.enabled = true;
        }
        AircraftPreset::Airship => {
            rig.optics.fov_y_deg = 30.0;
            rig.optics.fstop = 11.0;
            rig.constraints.ground_clearance_m = 160.0;
            rig.constraints.enabled = true;
        }
        AircraftPreset::FighterJet => {
            rig.optics.fov_y_deg = 62.0;
            rig.optics.fstop = 4.0;
            rig.constraints.ground_clearance_m = 30.0;
            rig.constraints.enabled = true;
        }
        AircraftPreset::Manual => {
            rig.optics.fov_y_deg = 50.0;
            rig.constraints.enabled = false;
        }
    }
}

/// Make sure an animation exists when the workspace is opened: enable the rig
/// and generate a starter orbit path if the project has no authored keys.
pub fn ensure_animation(rig: &mut CameraRig, hf: &HeightField) -> bool {
    let mut changed = false;
    let was_disabled = !rig.enabled;
    if was_disabled {
        rig.enabled = true;
        changed = true;
    }
    let untouched_default = rig.eye == Path3D::default();
    if (rig.eye.keys.len() < 2 || (was_disabled && untouched_default))
        && vf_flight::rig::preset_path(rig, hf, "demo").is_ok()
    {
        changed = true;
    }
    if rig.duration_s < rig.eye.duration_s() {
        rig.duration_s = rig.eye.duration_s().clamp(0.1, 600.0);
        changed = true;
    }
    changed
}

/// A point on the sample curve of a motion profile.
#[derive(Debug, Clone, Copy)]
pub struct ProfilePoint {
    pub time: f32,
    pub speed: f32,
    pub bank: f32,
}

#[derive(Debug, Clone)]
pub struct ProfileCurve {
    pub preset: AircraftPreset,
    pub points: Vec<ProfilePoint>,
}

#[derive(Debug, Clone, Default)]
pub struct ProfileCache {
    pub revision: u64,
    pub curves: Vec<ProfileCurve>,
}

/// Compare the six flight bodies on the current authored path.
pub fn profile_curves(rig: &CameraRig, heightfield: &HeightField, seed: Seed) -> Vec<ProfileCurve> {
    let mut curves = Vec::with_capacity(AircraftPreset::ALL.len());
    for preset in AircraftPreset::ALL {
        let mut candidate = rig.clone();
        apply_aircraft_preset(&mut candidate, preset);
        candidate.enabled = true;
        // The wall field is a 2D distance to steep cells; on a demo path that
        // flies high above a cliff it would push the comparison sideways even
        // though there is no collision. Keep the authored constraints untouched
        // and isolate body dynamics here.
        candidate.constraints.wall_avoidance = false;
        if candidate.validate().is_err() {
            continue;
        }
        let Ok(mut simulation) = RigSimulation::new(&candidate, heightfield, seed) else {
            continue;
        };
        let samples = 72;
        let mut points = Vec::with_capacity(samples + 1);
        for index in 0..=samples {
            let t = candidate.duration_s * index as f32 / samples as f32;
            let sample = simulation.sample_at(t);
            points.push(ProfilePoint {
                time: t,
                speed: sample.speed_mps,
                bank: sample.bank_deg,
            });
        }
        curves.push(ProfileCurve { preset, points });
    }
    curves
}

/// Simulated eye trajectory and authored guide for the active rig.
pub fn trail(rig: &CameraRig, heightfield: &HeightField, seed: Seed) -> Vec<[f64; 3]> {
    let mut trail = Vec::with_capacity(96);
    if let Ok(mut simulation) = RigSimulation::new(rig, heightfield, seed) {
        for index in 0..=96 {
            let t = rig.duration_s * index as f32 / 96.0;
            let sample = simulation.sample_at(t);
            trail.push(sample.eye.to_array());
        }
    }
    trail
}

/// One simulated trajectory in the profile comparison overlay.
#[derive(Debug, Clone)]
pub struct ComparisonTrail {
    pub preset: AircraftPreset,
    pub points: Vec<[f64; 3]>,
}

/// Simulate every body on the same authored path. Manual is skipped: it
/// follows the curve exactly and would just duplicate the path.
pub fn comparison_trails(
    rig: &CameraRig,
    heightfield: &HeightField,
    seed: Seed,
) -> Vec<ComparisonTrail> {
    let mut trails = Vec::with_capacity(AircraftPreset::ALL.len());
    for preset in AircraftPreset::ALL {
        if preset == AircraftPreset::Manual {
            continue;
        }
        let mut candidate = rig.clone();
        apply_aircraft_preset(&mut candidate, preset);
        candidate.enabled = true;
        // The wall field is a 2D distance to steep cells; on a demo path that
        // flies high above a cliff it would push the comparison sideways even
        // though there is no collision. Keep the authored constraints untouched
        // and isolate body dynamics here.
        candidate.constraints.wall_avoidance = false;
        if candidate.validate().is_err() {
            continue;
        }
        let Ok(mut simulation) = RigSimulation::new(&candidate, heightfield, seed) else {
            continue;
        };
        let samples = 72;
        let mut points = Vec::with_capacity(samples + 1);
        for index in 0..=samples {
            let t = candidate.duration_s * index as f32 / samples as f32;
            points.push(simulation.sample_at(t).eye.to_array());
        }
        trails.push(ComparisonTrail { preset, points });
    }
    trails
}

/// Duration guidance for the current path and body.
#[derive(Debug, Clone, Copy)]
pub struct TimingFit {
    pub length_m: f64,
    pub required_speed_mps: f32,
    pub cruise_speed_mps: f32,
    pub max_speed_mps: f32,
    pub recommended_duration_s: f32,
    pub pressure: f32,
}

impl TimingFit {
    pub fn measure(rig: &CameraRig) -> Self {
        Self {
            length_m: vf_flight::rig::path_length(&rig.eye),
            required_speed_mps: vf_flight::rig::required_speed_mps(rig),
            cruise_speed_mps: vf_flight::rig::cruise_speed_mps(&rig.body),
            max_speed_mps: rig.body.max_speed_mps,
            recommended_duration_s: vf_flight::rig::recommended_duration_s(rig),
            pressure: vf_flight::rig::duration_pressure(rig),
        }
    }

    /// True when the body will clearly saturate and lag behind the path.
    pub fn is_strained(self) -> bool {
        self.pressure > 1.6
    }

    pub fn advice(self) -> String {
        if self.length_m < 1.0 {
            "Le tracé est trop court pour être mesuré.".into()
        } else if self.is_strained() {
            format!(
                "Durée trop courte : {:.0} m/s demandés pour un maximum de {:.0} m/s. Le corps va saturer et prendre du retard. Essayez ≈ {:.0} s.",
                self.required_speed_mps, self.max_speed_mps, self.recommended_duration_s
            )
        } else if self.pressure < 0.45 {
            format!(
                "Durée très longue : le tracé ne fait que {:.0} m. Une durée de ≈ {:.0} s serait plus dynamique.",
                self.length_m, self.recommended_duration_s
            )
        } else {
            format!(
                "Bon calage : {:.0} m parcourus, {:.0} m/s moyens (croisière ≈ {:.0} m/s).",
                self.length_m, self.required_speed_mps, self.cruise_speed_mps
            )
        }
    }
}

/// Project a world point with the current perspective camera.
pub fn world_to_screen(camera: &Camera, rect: Rect, point: DVec3) -> Option<Pos2> {
    let aspect = f64::from(rect.width().max(1.0) / rect.height().max(1.0));
    let view_proj = camera.projection_matrix(aspect) * camera.view_matrix();
    let clip = view_proj * point.extend(1.0);
    if clip.w <= 1e-6 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if !ndc.x.is_finite() || !ndc.y.is_finite() {
        return None;
    }
    Some(Pos2::new(
        rect.left() + ((ndc.x * 0.5 + 0.5) as f32) * rect.width(),
        rect.top() + ((1.0 - (ndc.y * 0.5 + 0.5)) as f32) * rect.height(),
    ))
}

pub fn screen_ray(camera: &Camera, rect: Rect, pointer: Pos2) -> Option<(DVec3, DVec3)> {
    let aspect = f64::from(rect.width().max(1.0) / rect.height().max(1.0));
    let view_proj = camera.projection_matrix(aspect) * camera.view_matrix();
    let inverse = view_proj.inverse();
    let x = f64::from((pointer.x - rect.left()) / rect.width().max(1.0)) * 2.0 - 1.0;
    let y = 1.0 - f64::from((pointer.y - rect.top()) / rect.height().max(1.0)) * 2.0;
    let near = inverse * DVec4::new(x, y, 0.0, 1.0);
    let far = inverse * DVec4::new(x, y, 1.0, 1.0);
    if near.w.abs() < 1e-9 || far.w.abs() < 1e-9 {
        return None;
    }
    let origin = near.truncate() / near.w;
    let end = far.truncate() / far.w;
    let direction = (end - origin).normalize_or_zero();
    if direction.length_squared() < 0.5 {
        return None;
    }
    Some((origin, direction))
}

/// Pick the terrain under the pointer with a ray march + bisection.
pub fn pick_terrain(
    camera: &Camera,
    rect: Rect,
    pointer: Pos2,
    heightfield: &HeightField,
) -> Option<DVec3> {
    let (origin, direction) = screen_ray(camera, rect, pointer)?;
    let bounds = heightfield.bounds();
    let extent = bounds.extent().max(1.0);
    let max_distance =
        (camera.position - DVec3::from_array(bounds.center())).length() + extent * 2.0;
    let steps = 384;
    let step = max_distance / steps as f64;
    let sample_ground = |p: DVec3| -> Option<f64> {
        let (ox, oy) = heightfield.origin();
        let (sx, sy) = heightfield.spacing();
        let gx = (p.x - ox) / sx;
        let gy = (p.y - oy) / sy;
        if gx < 0.0
            || gy < 0.0
            || gx > (heightfield.width() - 1) as f64
            || gy > (heightfield.height() - 1) as f64
        {
            return None;
        }
        Some(f64::from(heightfield.sample_bilinear(gx, gy)))
    };
    let mut previous = None;
    for index in 0..=steps {
        let distance = index as f64 * step;
        let p = origin + direction * distance;
        let Some(ground) = sample_ground(p) else {
            previous = None;
            continue;
        };
        let delta = p.z - ground;
        if let Some((prev_delta, prev_p)) = previous
            && prev_delta > 0.0
            && delta <= 0.0
        {
            // Bisect between the two samples.
            let mut lo = prev_p;
            let mut hi = p;
            for _ in 0..22 {
                let mid = (lo + hi) * 0.5;
                if let Some(g) = sample_ground(mid) {
                    if mid.z - g > 0.0 {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                } else {
                    break;
                }
            }
            return Some(hi);
        }
        previous = Some((delta, p));
    }
    None
}

/// Lightweight chart of speed and bank for the profile comparison.
pub fn profile_chart(
    ui: &mut egui::Ui,
    curves: &[ProfileCurve],
    active: AircraftPreset,
) -> egui::Response {
    let width = ui.available_width().max(120.0);
    let height = 150.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, height), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 6.0, Color32::from_rgb(14, 18, 25));
    painter.rect_stroke(
        rect,
        6.0,
        Stroke::new(1.0, Color32::from_rgb(45, 58, 74)),
        egui::StrokeKind::Inside,
    );
    if curves.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Pas encore de courbes",
            egui::FontId::proportional(11.0),
            Color32::GRAY,
        );
        return response;
    }
    let speed_rect = Rect::from_min_max(
        Pos2::new(rect.left() + 36.0, rect.top() + 20.0),
        Pos2::new(rect.right() - 8.0, rect.top() + height * 0.52),
    );
    let bank_rect = Rect::from_min_max(
        Pos2::new(rect.left() + 36.0, rect.top() + height * 0.60),
        Pos2::new(rect.right() - 8.0, rect.bottom() - 16.0),
    );
    let duration = curves
        .iter()
        .flat_map(|curve| curve.points.iter().map(|p| p.time))
        .fold(0.0f32, f32::max)
        .max(0.1);
    let max_speed = curves
        .iter()
        .filter(|curve| curve.preset != AircraftPreset::Manual)
        .flat_map(|curve| curve.points.iter().map(|p| p.speed))
        .fold(1.0f32, f32::max)
        .max(1.0);
    let max_bank = curves
        .iter()
        .flat_map(|curve| curve.points.iter().map(|p| p.bank.abs()))
        .fold(1.0f32, f32::max)
        .max(1.0);

    for (label, area, top_value, bottom_value) in [
        ("vitesse m/s", speed_rect, max_speed, 0.0),
        ("roulis °", bank_rect, max_bank, -max_bank),
    ] {
        painter.line_segment(
            [area.left_bottom(), area.right_bottom()],
            Stroke::new(1.0, Color32::from_rgb(46, 57, 70)),
        );
        painter.line_segment(
            [area.left_top(), area.left_bottom()],
            Stroke::new(1.0, Color32::from_rgb(46, 57, 70)),
        );
        for fraction in [0.25f32, 0.5, 0.75] {
            let y = area.top() + area.height() * fraction;
            painter.line_segment(
                [Pos2::new(area.left(), y), Pos2::new(area.right(), y)],
                Stroke::new(0.6, Color32::from_rgb(30, 39, 50)),
            );
        }
        painter.text(
            Pos2::new(area.left() - 4.0, area.top()),
            egui::Align2::RIGHT_TOP,
            format!("{top_value:.0}"),
            egui::FontId::monospace(8.0),
            Color32::from_rgb(150, 165, 180),
        );
        painter.text(
            Pos2::new(area.left() - 4.0, area.bottom()),
            egui::Align2::RIGHT_BOTTOM,
            format!("{bottom_value:.0}"),
            egui::FontId::monospace(8.0),
            Color32::from_rgb(150, 165, 180),
        );
        painter.text(
            Pos2::new(area.left(), area.top() - 2.0),
            egui::Align2::LEFT_BOTTOM,
            label,
            egui::FontId::proportional(9.0),
            Color32::from_rgb(160, 175, 192),
        );
    }

    for curve in curves {
        // Manual follows the authored speed exactly and would flatten the
        // scale; it is listed in the legend but not plotted.
        if curve.preset == AircraftPreset::Manual {
            continue;
        }
        let color = preset_color(curve.preset);
        let active_curve = curve.preset == active;
        let stroke = Stroke::new(if active_curve { 2.2 } else { 1.1 }, color);
        let mut speed_points = Vec::with_capacity(curve.points.len());
        let mut bank_points = Vec::with_capacity(curve.points.len());
        for point in &curve.points {
            let x =
                speed_rect.left() + (point.time / duration).clamp(0.0, 1.0) * speed_rect.width();
            let y_speed = speed_rect.bottom()
                - (point.speed / max_speed).clamp(0.0, 1.0) * speed_rect.height();
            speed_points.push(Pos2::new(x, y_speed));
            let normalized_bank = (point.bank + max_bank) / (2.0 * max_bank);
            let y_bank = bank_rect.bottom() - normalized_bank.clamp(0.0, 1.0) * bank_rect.height();
            bank_points.push(Pos2::new(x, y_bank));
        }
        painter.add(egui::Shape::line(speed_points, stroke));
        painter.add(egui::Shape::line(bank_points, stroke));
    }
    painter.text(
        Pos2::new(rect.right() - 6.0, rect.top() + 5.0),
        egui::Align2::RIGHT_TOP,
        format!("0 → {duration:.1} s"),
        egui::FontId::monospace(8.0),
        Color32::from_rgb(130, 145, 160),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::{CameraRig, HeightField, PathKey, TargetMode};

    fn flat_field() -> HeightField {
        HeightField::filled(65, 65, 10.0, 0.0).unwrap()
    }

    fn demo_rig() -> CameraRig {
        let mut rig = CameraRig::preset("drone").unwrap();
        rig.enabled = true;
        rig.duration_s = 12.0;
        rig.eye.keys = vec![
            PathKey::new(0.0, [100.0, 100.0, 80.0]),
            PathKey::new(6.0, [320.0, 320.0, 120.0]),
            PathKey::new(12.0, [540.0, 540.0, 80.0]),
        ];
        rig.target.mode = TargetMode::Fixed;
        rig.target.fixed = [320.0, 320.0, 40.0];
        rig.constraints.enabled = false;
        rig
    }

    #[test]
    fn timing_fit_recommends_a_comfortable_duration() {
        let mut rig = demo_rig();
        rig.duration_s = 2.0;
        let fit = TimingFit::measure(&rig);
        assert!(fit.is_strained());
        rig.duration_s = fit.recommended_duration_s;
        let relaxed = TimingFit::measure(&rig);
        assert!(!relaxed.is_strained(), "{relaxed:?}");
        assert!((relaxed.pressure - 1.0).abs() < 0.1);
    }

    #[test]
    fn comparison_trails_are_deterministic_and_cover_the_bodies() {
        let rig = demo_rig();
        let hf = flat_field();
        let first = comparison_trails(&rig, &hf, Seed::new(3));
        let second = comparison_trails(&rig, &hf, Seed::new(3));
        assert_eq!(first.len(), 5, "Manual is intentionally excluded");
        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(second.iter()) {
            assert_eq!(a.preset, b.preset);
            assert_eq!(a.points, b.points);
            assert_eq!(a.points.len(), 73);
        }
        assert!(
            first
                .iter()
                .any(|trail| trail.preset == AircraftPreset::FighterJet)
        );
        assert!(
            !first
                .iter()
                .any(|trail| trail.preset == AircraftPreset::Manual)
        );
    }

    #[test]
    fn profile_curves_show_a_speed_difference_between_fast_and_slow_bodies() {
        let rig = demo_rig();
        let hf = flat_field();
        let curves = profile_curves(&rig, &hf, Seed::new(1));
        let max_speed = |preset: AircraftPreset| {
            curves
                .iter()
                .find(|curve| curve.preset == preset)
                .map(|curve| {
                    curve
                        .points
                        .iter()
                        .map(|point| point.speed)
                        .fold(0.0f32, f32::max)
                })
                .unwrap_or(0.0)
        };
        assert!(max_speed(AircraftPreset::FighterJet) > max_speed(AircraftPreset::Airship) + 2.0);
    }
}
