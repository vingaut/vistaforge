//! CAD-style orientation gizmo.
//!
//! The widget draws a small view cube surrounded by the three world axes. It
//! is an overlay: clicking a face or an axis endpoint snaps the camera to that
//! standard view, clicking the house returns to the default three-quarter
//! framing. Faces are painted with the same camera basis as the viewport, so
//! the cube reads as a miniature of the scene orientation.

use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use glam::DVec3;
use vf_core::Camera;
use vf_scene::ViewPreset;

/// Preferred size of the gizmo overlay, in points.
pub const GIZMO_SIZE: Vec2 = Vec2::new(146.0, 190.0);
const GIZMO_CENTER_Y: f32 = 86.0;
const MARGIN: f32 = 12.0;
const CUBE_HALF: f32 = 22.0;
const AXIS_LENGTH: f32 = 2.4;
const AXIS_RADIUS: f32 = 7.0;
const NEGATIVE_AXIS_RADIUS: f32 = 5.0;

/// Action requested by a click on the gizmo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GizmoAction {
    View(ViewPreset),
    /// Return to the default auto-framed three-quarter view.
    Home,
}

impl GizmoAction {
    pub fn tooltip(self) -> &'static str {
        match self {
            Self::View(ViewPreset::Perspective) => "Vue en perspective",
            Self::View(ViewPreset::Front) => "Vue de face",
            Self::View(ViewPreset::Back) => "Vue arrière",
            Self::View(ViewPreset::Left) => "Vue de gauche",
            Self::View(ViewPreset::Right) => "Vue de droite",
            Self::View(ViewPreset::Top) => "Vue de dessus",
            Self::View(ViewPreset::Bottom) => "Vue de dessous",
            Self::Home => "Vue par défaut",
        }
    }
}

/// Result returned by [`show`].
#[derive(Debug, Clone)]
pub struct GizmoResponse {
    pub action: Option<GizmoAction>,
    /// Target under the pointer. The viewport uses it as a fallback when the
    /// full-surface viewport response wins the click hit-test.
    pub hovered: Option<GizmoAction>,
}

/// Compute the overlay rectangle, or `None` when the viewport is too small.
pub fn rect_for(viewport: Rect) -> Option<Rect> {
    if viewport.width() < 230.0 || viewport.height() < 220.0 {
        return None;
    }
    Some(Rect::from_min_size(
        Pos2::new(
            viewport.right() - GIZMO_SIZE.x - MARGIN,
            viewport.top() + MARGIN,
        ),
        GIZMO_SIZE,
    ))
}

/// Draw and interact with the gizmo.
pub fn show(ui: &mut egui::Ui, viewport: Rect, camera: &Camera) -> GizmoResponse {
    let Some(rect) = rect_for(viewport) else {
        return GizmoResponse {
            action: None,
            hovered: None,
        };
    };
    let response = ui.interact(rect, ui.scope_id().with("view-gizmo"), Sense::click());

    let forward = camera.forward();
    let right = camera.right();
    let up = camera.true_up();
    if forward.length_squared() < 0.5 || right.length_squared() < 0.5 || up.length_squared() < 0.5 {
        return GizmoResponse {
            action: None,
            hovered: None,
        };
    }

    let center = Pos2::new(rect.center().x, rect.top() + GIZMO_CENTER_Y);
    let project = |p: DVec3| -> Pos2 {
        center
            + Vec2::new(
                p.dot(right) as f32 * CUBE_HALF,
                -p.dot(up) as f32 * CUBE_HALF,
            )
    };

    let faces = project_faces(&project, forward);
    let axes = project_axes(&project, forward);
    let home_rect = home_rect(rect);
    let pointer = response.hover_pos();
    let hovered =
        pointer.and_then(|p| hit_test(p, &faces, &axes, home_rect, pointer_is_in_rect(p, rect)));

    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    // Panel.
    painter.rect_filled(
        rect,
        egui::CornerRadius::same(10),
        Color32::from_rgba_unmultiplied(13, 17, 24, 205),
    );
    painter.rect_stroke(
        rect,
        egui::CornerRadius::same(10),
        Stroke::new(1.0, Color32::from_rgba_unmultiplied(120, 145, 175, 90)),
        egui::StrokeKind::Inside,
    );

    // Current view caption.
    let current = facing(forward);
    let caption = current.map_or("Perspective", ViewPreset::label);
    painter.text(
        Pos2::new(rect.center().x, rect.top() + 12.0),
        egui::Align2::CENTER_TOP,
        caption,
        FontId::proportional(10.0),
        Color32::from_rgba_unmultiplied(210, 225, 240, 185),
    );

    // Shafts first: the cube covers their roots.
    for axis in &axes {
        let end = axis.endpoint;
        let color = if axis.negative {
            axis.color.gamma_multiply(0.55)
        } else {
            axis.color.gamma_multiply(0.8)
        };
        painter.line_segment(
            [center, end],
            Stroke::new(if axis.negative { 1.0 } else { 1.6 }, color),
        );
    }

    // Cube faces, far to near. Visible faces never overlap, but the ordering
    // keeps the text readable if they share an edge.
    let mut ordered: Vec<&ProjectedFace> = faces.iter().collect();
    ordered.sort_by(|a, b| b.depth.total_cmp(&a.depth));
    for face in ordered {
        let is_hovered = hovered == Some(GizmoAction::View(face.preset));
        let is_current = current == Some(face.preset);
        let fill = if is_hovered {
            Color32::from_rgb(92, 145, 205)
        } else if is_current {
            face.fill.gamma_multiply(1.45)
        } else {
            face.fill
        };
        let stroke = Stroke::new(
            if is_hovered { 1.6 } else { 1.0 },
            if is_hovered {
                Color32::from_rgb(225, 240, 255)
            } else {
                Color32::from_rgba_unmultiplied(165, 190, 220, 170)
            },
        );
        painter.add(egui::Shape::convex_polygon(
            face.polygon.to_vec(),
            fill,
            stroke,
        ));

        if face.area > 120.0 && face.polygon.iter().all(|p| rect.contains(*p)) {
            let width = face
                .polygon
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
                    (lo.min(p.x), hi.max(p.x))
                });
            let available = width.1 - width.0 - 3.0;
            let color = Color32::from_rgba_unmultiplied(235, 243, 255, 220);
            let mut chosen = None;
            for size in [8.0, 7.0, 6.0] {
                let font = FontId::proportional(size);
                let galley = painter.layout_no_wrap(face.label.to_owned(), font, color);
                if galley.size().x <= available {
                    chosen = Some(galley);
                    break;
                }
            }
            if let Some(galley) = chosen {
                let pos = face.center - galley.size() * 0.5;
                painter.galley(pos, galley, color);
            }
        }
    }

    // Axis endpoints.
    for axis in &axes {
        let is_hovered = hovered == Some(GizmoAction::View(axis.preset));
        let radius = if axis.negative {
            NEGATIVE_AXIS_RADIUS
        } else {
            AXIS_RADIUS
        };
        if is_hovered {
            painter.circle_filled(axis.endpoint, radius + 3.0, axis.color.gamma_multiply(0.35));
            painter.circle_stroke(
                axis.endpoint,
                radius + 3.0,
                Stroke::new(1.5, Color32::WHITE),
            );
        }
        if axis.negative {
            painter.circle_filled(
                axis.endpoint,
                radius,
                Color32::from_rgba_unmultiplied(22, 27, 36, 230),
            );
            painter.circle_stroke(axis.endpoint, radius, Stroke::new(1.4, axis.color));
            painter.circle_filled(axis.endpoint, 1.3, axis.color);
        } else {
            painter.circle_filled(axis.endpoint, radius, axis.color);
            painter.circle_stroke(
                axis.endpoint,
                radius,
                Stroke::new(1.0, Color32::from_rgba_unmultiplied(16, 20, 28, 180)),
            );
            painter.text(
                axis.endpoint,
                egui::Align2::CENTER_CENTER,
                axis.label,
                FontId::monospace(9.0),
                Color32::from_rgb(16, 20, 28),
            );
        }
    }

    // Home button.
    let home_hovered = hovered == Some(GizmoAction::Home);
    let home_fill = if home_hovered {
        Color32::from_rgb(72, 104, 146)
    } else {
        Color32::from_rgba_unmultiplied(38, 47, 61, 235)
    };
    painter.rect_filled(home_rect, egui::CornerRadius::same(6), home_fill);
    painter.rect_stroke(
        home_rect,
        egui::CornerRadius::same(6),
        Stroke::new(
            1.0,
            if home_hovered {
                Color32::from_rgb(220, 236, 255)
            } else {
                Color32::from_rgba_unmultiplied(130, 155, 185, 120)
            },
        ),
        egui::StrokeKind::Inside,
    );
    draw_house(&painter, home_rect, home_hovered);
    painter.text(
        Pos2::new(home_rect.center().x + 7.0, home_rect.center().y),
        egui::Align2::CENTER_CENTER,
        "Défaut",
        FontId::proportional(10.0),
        Color32::from_rgba_unmultiplied(230, 240, 252, 230),
    );

    if response.hovered() && hovered.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let action = if response.clicked() { hovered } else { None };

    // Tooltip on the target under the pointer. `on_hover_text` is only shown
    // when egui considers the response hovered, which is what we want.
    if let Some(target) = hovered {
        response
            .clone()
            .on_hover_text(format!("{} (clic)", target.tooltip()));
    }

    GizmoResponse { action, hovered }
}

#[derive(Debug, Clone, Copy)]
struct AxisBall {
    endpoint: Pos2,
    preset: ViewPreset,
    label: &'static str,
    color: Color32,
    negative: bool,
}

#[derive(Debug, Clone)]
struct ProjectedFace {
    preset: ViewPreset,
    polygon: [Pos2; 4],
    center: Pos2,
    area: f32,
    depth: f32,
    fill: Color32,
    label: &'static str,
}

fn project_faces(project: &impl Fn(DVec3) -> Pos2, forward: DVec3) -> Vec<ProjectedFace> {
    let mut faces = Vec::with_capacity(3);
    for preset in ViewPreset::ALL {
        let Some(normal) = normal_for(preset) else {
            continue;
        };
        // A face is visible when its outward normal points toward the camera.
        if normal.dot(forward) > -0.06 {
            continue;
        }
        let (u, v) = tangent_axes(normal);
        let corners = [
            project(normal + u + v),
            project(normal - u + v),
            project(normal - u - v),
            project(normal + u - v),
        ];
        let area = polygon_area(&corners);
        if area < 5.0 {
            continue;
        }
        let center = average_point(&corners);
        faces.push(ProjectedFace {
            preset,
            polygon: corners,
            center,
            area,
            depth: normal.dot(forward) as f32,
            fill: face_fill(normal),
            label: face_label(preset),
        });
    }
    faces
}

fn project_axes(project: &impl Fn(DVec3) -> Pos2, _forward: DVec3) -> Vec<AxisBall> {
    [
        (
            DVec3::X,
            "X",
            ViewPreset::Right,
            Color32::from_rgb(232, 96, 96),
            false,
        ),
        (
            DVec3::NEG_X,
            "−X",
            ViewPreset::Left,
            Color32::from_rgb(232, 96, 96),
            true,
        ),
        (
            DVec3::Y,
            "Y",
            ViewPreset::Back,
            Color32::from_rgb(96, 200, 125),
            false,
        ),
        (
            DVec3::NEG_Y,
            "−Y",
            ViewPreset::Front,
            Color32::from_rgb(96, 200, 125),
            true,
        ),
        (
            DVec3::Z,
            "Z",
            ViewPreset::Top,
            Color32::from_rgb(86, 146, 255),
            false,
        ),
        (
            DVec3::NEG_Z,
            "−Z",
            ViewPreset::Bottom,
            Color32::from_rgb(86, 146, 255),
            true,
        ),
    ]
    .into_iter()
    .map(|(dir, label, preset, color, negative)| AxisBall {
        endpoint: project(dir * f64::from(AXIS_LENGTH)),
        preset,
        label,
        color,
        negative,
    })
    .collect()
}

fn hit_test(
    pointer: Pos2,
    faces: &[ProjectedFace],
    axes: &[AxisBall],
    home: Rect,
    inside: bool,
) -> Option<GizmoAction> {
    if !inside {
        return None;
    }
    if home.contains(pointer) {
        return Some(GizmoAction::Home);
    }
    // Faces take priority over axis endpoints: in a top or bottom view the
    // axis pointing at the camera projects onto the cube centre, and the
    // hidden opposite endpoint must not steal the click.
    if let Some(face) = faces
        .iter()
        .filter(|face| point_in_polygon(pointer, &face.polygon))
        .min_by(|a, b| a.depth.total_cmp(&b.depth))
    {
        return Some(GizmoAction::View(face.preset));
    }
    for axis in axes {
        let radius = if axis.negative {
            NEGATIVE_AXIS_RADIUS
        } else {
            AXIS_RADIUS
        } + 4.0;
        if axis.endpoint.distance(pointer) <= radius {
            return Some(GizmoAction::View(axis.preset));
        }
    }
    None
}

fn pointer_is_in_rect(pointer: Pos2, rect: Rect) -> bool {
    rect.contains(pointer)
}

fn home_rect(gizmo: Rect) -> Rect {
    Rect::from_center_size(
        Pos2::new(gizmo.center().x, gizmo.bottom() - 19.0),
        Vec2::new(96.0, 22.0),
    )
}

fn draw_house(painter: &egui::Painter, button: Rect, hovered: bool) {
    let color = if hovered {
        Color32::WHITE
    } else {
        Color32::from_rgba_unmultiplied(205, 220, 238, 220)
    };
    let x = button.left() + 15.0;
    let y = button.center().y;
    let stroke = Stroke::new(1.4, color);
    painter.line_segment([Pos2::new(x - 6.0, y), Pos2::new(x, y - 5.0)], stroke);
    painter.line_segment([Pos2::new(x, y - 5.0), Pos2::new(x + 6.0, y)], stroke);
    painter.line_segment(
        [Pos2::new(x - 4.0, y - 0.5), Pos2::new(x - 4.0, y + 5.0)],
        stroke,
    );
    painter.line_segment(
        [Pos2::new(x + 4.0, y - 0.5), Pos2::new(x + 4.0, y + 5.0)],
        stroke,
    );
    painter.line_segment(
        [Pos2::new(x - 4.0, y + 5.0), Pos2::new(x + 4.0, y + 5.0)],
        stroke,
    );
}

fn facing(forward: DVec3) -> Option<ViewPreset> {
    ViewPreset::from_normal(-forward)
}

fn normal_for(preset: ViewPreset) -> Option<DVec3> {
    Some(match preset {
        ViewPreset::Perspective => return None,
        ViewPreset::Front => DVec3::NEG_Y,
        ViewPreset::Back => DVec3::Y,
        ViewPreset::Left => DVec3::NEG_X,
        ViewPreset::Right => DVec3::X,
        ViewPreset::Top => DVec3::Z,
        ViewPreset::Bottom => DVec3::NEG_Z,
    })
}

fn tangent_axes(normal: DVec3) -> (DVec3, DVec3) {
    if normal.x.abs() > 0.5 {
        (DVec3::Y, DVec3::Z)
    } else if normal.y.abs() > 0.5 {
        (DVec3::X, DVec3::Z)
    } else {
        (DVec3::X, DVec3::Y)
    }
}

fn face_label(preset: ViewPreset) -> &'static str {
    match preset {
        ViewPreset::Perspective => "PERSP",
        ViewPreset::Front => "FACE",
        ViewPreset::Back => "DOS",
        ViewPreset::Left => "GAUCHE",
        ViewPreset::Right => "DROITE",
        ViewPreset::Top => "HAUT",
        ViewPreset::Bottom => "BAS",
    }
}

fn face_fill(normal: DVec3) -> Color32 {
    if normal.x.abs() > 0.5 {
        Color32::from_rgb(72, 52, 58)
    } else if normal.y.abs() > 0.5 {
        Color32::from_rgb(49, 70, 58)
    } else {
        Color32::from_rgb(48, 63, 88)
    }
}

fn polygon_area(points: &[Pos2; 4]) -> f32 {
    let mut area = 0.0;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        area += a.x * b.y - b.x * a.y;
    }
    area.abs() * 0.5
}

fn average_point(points: &[Pos2; 4]) -> Pos2 {
    let sum = points.iter().fold(Vec2::ZERO, |acc, p| acc + p.to_vec2());
    Pos2::new(sum.x / 4.0, sum.y / 4.0)
}

fn point_in_polygon(pointer: Pos2, polygon: &[Pos2]) -> bool {
    let mut inside = false;
    let mut j = polygon.len() - 1;
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[j];
        if (a.y > pointer.y) != (b.y > pointer.y) {
            let x = (b.x - a.x) * (pointer.y - a.y) / (b.y - a.y) + a.x;
            if pointer.x < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_normals_round_trip_to_presets() {
        for preset in ViewPreset::ALL {
            if let Some(normal) = normal_for(preset) {
                assert_eq!(ViewPreset::from_normal(normal), Some(preset));
            }
        }
    }

    #[test]
    fn top_face_is_seen_from_a_top_camera() {
        let forward = DVec3::NEG_Z;
        let project = |p: DVec3| Pos2::new(50.0 + p.x as f32 * 20.0, 50.0 - p.y as f32 * 20.0);
        let faces = project_faces(&project, forward);
        assert!(faces.iter().any(|face| face.preset == ViewPreset::Top));
        assert!(!faces.iter().any(|face| face.preset == ViewPreset::Front));
        assert_eq!(facing(forward), Some(ViewPreset::Top));
    }

    #[test]
    fn axis_endpoints_are_outside_the_cube() {
        let project = |p: DVec3| Pos2::new(p.x as f32 * 20.0, -p.y as f32 * 20.0);
        let axes = project_axes(&project, DVec3::NEG_Z);
        assert_eq!(axes.len(), 6);
        let x = axes.iter().find(|axis| axis.label == "X").unwrap();
        assert!(x.endpoint.x > CUBE_HALF);
    }

    #[test]
    fn point_inside_polygon() {
        let polygon = [
            Pos2::new(0.0, 0.0),
            Pos2::new(10.0, 0.0),
            Pos2::new(10.0, 10.0),
            Pos2::new(0.0, 10.0),
        ];
        assert!(point_in_polygon(Pos2::new(5.0, 5.0), &polygon));
        assert!(!point_in_polygon(Pos2::new(15.0, 5.0), &polygon));
    }
}

#[cfg(test)]
mod click_tests {
    use super::*;
    use vf_core::HeightField;

    fn camera(preset: ViewPreset) -> Camera {
        let heightfield = HeightField::new(65, 65, 10.0, 10.0, vec![0.0; 65 * 65]).unwrap();
        vf_scene::framed_view(&heightfield, 4.0 / 3.0, preset, 48.0)
    }

    fn run_frame(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        camera: &Camera,
        pointer: Pos2,
    ) -> Option<GizmoAction> {
        let mut result = None;
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))),
            events,
            ..Default::default()
        };
        let output = ctx.run_ui(raw, |ui| {
            let viewport = ui.max_rect();
            // Mimic the application's full-surface viewport response.
            let viewport_response = ui.allocate_rect(viewport, Sense::click_and_drag());
            let gizmo = show(ui, viewport, camera);
            let over_gizmo = rect_for(viewport).is_some_and(|rect| rect.contains(pointer));
            result = gizmo.action.or_else(|| {
                if viewport_response.clicked() && over_gizmo {
                    gizmo.hovered
                } else {
                    None
                }
            });
        });
        output.drop_without_applying_deltas();
        result
    }

    fn click(ctx: &egui::Context, camera: &Camera, point: Pos2) -> Option<GizmoAction> {
        let _ = run_frame(ctx, vec![egui::Event::PointerMoved(point)], camera, point);
        let _ = run_frame(
            ctx,
            vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            camera,
            point,
        );
        run_frame(
            ctx,
            vec![egui::Event::PointerButton {
                pos: point,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
            camera,
            point,
        )
    }

    #[test]
    fn clicking_the_top_axis_snaps_to_top() {
        let ctx = egui::Context::default();
        let camera = camera(ViewPreset::Front);
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let gizmo = rect_for(viewport).unwrap();
        let point = Pos2::new(
            gizmo.center().x,
            gizmo.top() + GIZMO_CENTER_Y - AXIS_LENGTH * CUBE_HALF,
        );
        assert_eq!(
            click(&ctx, &camera, point),
            Some(GizmoAction::View(ViewPreset::Top))
        );
    }

    #[test]
    fn clicking_the_center_in_bottom_view_keeps_bottom() {
        let ctx = egui::Context::default();
        let camera = camera(ViewPreset::Bottom);
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let gizmo = rect_for(viewport).unwrap();
        let point = Pos2::new(gizmo.center().x, gizmo.top() + GIZMO_CENTER_Y);
        assert_eq!(
            click(&ctx, &camera, point),
            Some(GizmoAction::View(ViewPreset::Bottom))
        );
    }

    #[test]
    fn clicking_home_returns_home_action() {
        let ctx = egui::Context::default();
        let camera = camera(ViewPreset::Front);
        let viewport = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let point = home_rect(rect_for(viewport).unwrap()).center();
        assert_eq!(click(&ctx, &camera, point), Some(GizmoAction::Home));
    }
}
