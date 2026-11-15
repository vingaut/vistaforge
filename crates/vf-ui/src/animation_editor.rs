//! 3D path editor overlay for the animation workspace.
//!
//! Draws the authored camera path, the target path and a translate gizmo on
//! the selected key. All interactions are screen-space hit-tests against
//! projected 3D points, so the editor works over both the GPU viewport and the
//! CPU preview texture. Camera orbit is left to the viewport; `consumed` tells
//! it to stay out of the way when the gizmo, a key or the trace tool is used.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use glam::DVec3;
use vf_core::{Camera, CameraRig, HeightField, PathKey, TargetMode};
use vf_flight::rig::{RigSample, path_derivative, sample_path};

use crate::animation_ui::{
    AnimSelection, ComparisonTrail, pick_terrain, preset_color, screen_ray, world_to_screen,
};

const EYE_COLOR: Color32 = Color32::from_rgb(92, 202, 255);
const TARGET_COLOR: Color32 = Color32::from_rgb(255, 176, 92);
const TRAIL_COLOR: Color32 = Color32::from_rgb(255, 226, 120);
const AXIS_COLORS: [Color32; 3] = [
    Color32::from_rgb(236, 92, 92),
    Color32::from_rgb(96, 208, 128),
    Color32::from_rgb(92, 148, 255),
];
const AXIS_LABELS: [&str; 3] = ["X", "Y", "Z"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    fn index(self) -> usize {
        match self {
            Self::X => 0,
            Self::Y => 1,
            Self::Z => 2,
        }
    }
}

struct AxisDrag {
    selection: AnimSelection,
    axis: Axis,
    start_pointer: Pos2,
    start_value: f64,
    world_length: f64,
    screen_dir: Vec2,
    screen_length: f32,
}

/// Free mouse drag: the key follows the pointer in the plane of the screen,
/// without requiring the user to grab a gizmo axis.
struct MarkerDrag {
    selection: AnimSelection,
    start_pointer: Pos2,
    start_position: DVec3,
    plane_normal: DVec3,
    world_per_pixel: f64,
    moved: bool,
}

#[derive(Default)]
pub struct EditorState {
    pub selection: Option<AnimSelection>,
    pub trace: bool,
    /// Add traced keys to the target path instead of the eye path.
    pub trace_target: bool,
    /// Height above the picked ground for new eye keys, metres.
    pub trace_altitude: f32,
    /// Grid snap in metres; 0 disables snapping.
    pub snap_m: f64,
    pub show_paths: bool,
    pub show_trail: bool,
    /// Click and drag directly on a key to move it in the screen plane.
    pub free_drag: bool,
    drag: Option<AxisDrag>,
    marker_drag: Option<MarkerDrag>,
    hover_axis: Option<Axis>,
    hover_marker: Option<AnimSelection>,
}

pub struct EditorOutput {
    pub changed: bool,
    /// The viewport should not orbit/zoom/click this frame.
    pub consumed: bool,
    pub status: Option<String>,
    pub last_added_time: Option<f32>,
}

impl EditorState {
    pub fn new() -> Self {
        Self {
            selection: None,
            trace: false,
            trace_target: false,
            trace_altitude: 120.0,
            snap_m: 0.0,
            show_paths: true,
            show_trail: true,
            free_drag: true,
            drag: None,
            marker_drag: None,
            hover_axis: None,
            hover_marker: None,
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some() || self.marker_drag.is_some()
    }

    pub fn clear_invalid_selection(&mut self, rig: &CameraRig) {
        let valid = match self.selection {
            Some(AnimSelection::EyeKey(index)) => index < rig.eye.keys.len(),
            Some(AnimSelection::TargetKey(index)) => {
                rig.target.mode == TargetMode::Path && index < rig.target.path.keys.len()
            }
            Some(AnimSelection::FixedTarget) => rig.target.mode == TargetMode::Fixed,
            None => true,
        };
        if !valid {
            self.selection = None;
        }
    }
}

/// Draw the path overlay and handle the translate gizmo / picking / tracing.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut egui::Ui,
    rect: Rect,
    camera: &Camera,
    rig: &mut CameraRig,
    heightfield: &HeightField,
    time: f32,
    state: &mut EditorState,
    simulated_trail: Option<&[[f64; 3]]>,
    comparison: Option<&[ComparisonTrail]>,
    live_sample: Option<&RigSample>,
    blocked_rect: Rect,
) -> EditorOutput {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let pointer = ui.input(|input| input.pointer.interact_pos());
    let pressed = ui.input(|input| input.pointer.primary_pressed());
    let down = ui.input(|input| input.pointer.primary_down());
    let clicked = ui.input(|input| input.pointer.primary_clicked());
    let mut output = EditorOutput {
        changed: false,
        consumed: false,
        status: None,
        last_added_time: None,
    };
    state.clear_invalid_selection(rig);

    if state.show_paths {
        if let Some(comparison) = comparison {
            for trail in comparison {
                // The active body is drawn in yellow by `draw_trail`; keeping
                // its preset colour here would just overdraw it.
                if trail.preset != rig.body.preset {
                    draw_comparison_trail(&painter, rect, camera, trail);
                }
            }
        }
        draw_path(&painter, rect, camera, &rig.eye, EYE_COLOR, 1.8, "Œil");
        if rig.target.mode == TargetMode::Path {
            draw_path(
                &painter,
                rect,
                camera,
                &rig.target.path,
                TARGET_COLOR,
                1.3,
                "Cible",
            );
        }
        if state.show_trail
            && let Some(trail) = simulated_trail
        {
            draw_trail(&painter, rect, camera, trail);
        }
        draw_camera_frustum(&painter, rect, camera, rig, time, live_sample);
        draw_time_markers(&painter, rect, camera, rig, time, live_sample);
    }

    // Selected key gizmo geometry. Drawn after paths and markers so it stays
    // on top, and hit-tested before markers so dragging an axis wins.
    let selected = selected_world_position(rig, state.selection);
    let gizmo = selected.map(|position| Gizmo::build(rect, camera, position));
    let over_blocked = blocked_rect.width() > 0.0
        && blocked_rect.height() > 0.0
        && blocked_rect.contains(pointer.unwrap_or(Pos2::ZERO));
    state.hover_axis =
        pointer.and_then(|pointer| gizmo.as_ref().and_then(|gizmo| gizmo.hit_axis(pointer)));
    state.hover_marker = if state.drag.is_none() && !over_blocked {
        pointer.and_then(|pointer| hit_marker(rect, camera, rig, pointer))
    } else {
        None
    };
    if state.hover_axis.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    } else if state.hover_marker.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    // Start a gizmo drag on press.
    if pressed
        && !over_blocked
        && let Some(pointer) = pointer
        && let Some(gizmo) = &gizmo
        && let Some(axis) = gizmo.hit_axis(pointer)
    {
        let start_value = selected_component(rig, state.selection, axis);
        state.drag = Some(AxisDrag {
            selection: state.selection.expect("gizmo has a selection"),
            axis,
            start_pointer: pointer,
            start_value,
            world_length: gizmo.world_length,
            screen_dir: gizmo.screen_dir(axis),
            screen_length: gizmo.screen_length(axis),
        });
    }

    if let Some(drag) = &state.drag {
        output.consumed = true;
        if let Some(pointer) = pointer {
            let delta_px = (pointer - drag.start_pointer).dot(drag.screen_dir);
            let mut value = drag.start_value
                + f64::from(delta_px) / f64::from(drag.screen_length.max(1.0)) * drag.world_length;
            if state.snap_m > 0.0 {
                value = (value / state.snap_m).round() * state.snap_m;
            }
            if set_component(rig, Some(drag.selection), drag.axis, value) {
                output.changed = true;
            }
        }
        if !down {
            state.drag = None;
            if output.changed {
                output.status = Some("Clé déplacée".into());
            }
        }
    }

    // Marker picking on press. A press also starts a free drag: the key
    // follows the pointer in the screen plane, so the gizmo is optional.
    if pressed
        && !state.trace
        && state.drag.is_none()
        && state.marker_drag.is_none()
        && !over_blocked
        && let Some(pointer) = pointer
    {
        let marker = hit_marker(rect, camera, rig, pointer);
        state.hover_marker = marker;
        if let Some(marker) = marker {
            state.selection = Some(marker);
            output.consumed = true;
            state.trace = false;
            output.status = Some(match marker {
                AnimSelection::EyeKey(index) => format!("Clé œil {index} sélectionnée"),
                AnimSelection::TargetKey(index) => format!("Clé cible {index} sélectionnée"),
                AnimSelection::FixedTarget => "Cible fixe sélectionnée".into(),
            });
            if state.free_drag
                && let Some(position) = selected_world_position(rig, Some(marker))
            {
                let forward = (camera.target - camera.position).normalize_or_zero();
                let distance = (position - camera.position).length().max(1.0);
                let world_per_pixel =
                    distance * (f64::from(camera.fov_y_deg) * 0.5).to_radians().tan() * 2.0
                        / f64::from(rect.height().max(1.0));
                state.marker_drag = Some(MarkerDrag {
                    selection: marker,
                    start_pointer: pointer,
                    start_position: position,
                    plane_normal: forward,
                    world_per_pixel,
                    moved: false,
                });
            }
        }
    }

    // Free marker drag in the screen plane. Shift locks Z, Alt snaps the key
    // to the terrain under the pointer.
    if let Some(drag) = state.marker_drag.as_mut() {
        output.consumed = true;
        if let Some(pointer) = pointer {
            let delta = pointer - drag.start_pointer;
            if delta.length() > 1.5 {
                drag.moved = true;
            }
            if drag.moved {
                let shift = ui.input(|input| input.modifiers.shift);
                let alt = ui.input(|input| input.modifiers.alt);
                let mut position = drag.start_position;
                if alt {
                    if let Some(hit) = pick_terrain(camera, rect, pointer, heightfield) {
                        position = hit;
                    }
                } else if shift {
                    position.z = drag.start_position.z - f64::from(delta.y) * drag.world_per_pixel;
                } else if let Some((origin, direction)) = screen_ray(camera, rect, pointer)
                    && let Some(hit) =
                        ray_plane(origin, direction, drag.start_position, drag.plane_normal)
                {
                    position = hit;
                }
                if state.snap_m > 0.0 {
                    position.x = (position.x / state.snap_m).round() * state.snap_m;
                    position.y = (position.y / state.snap_m).round() * state.snap_m;
                    position.z = (position.z / state.snap_m).round() * state.snap_m;
                }
                if set_position(rig, Some(drag.selection), position) {
                    output.changed = true;
                    output.status = Some(format!(
                        "Clé déplacée librement · ({:.1}, {:.1}, {:.1})",
                        position.x, position.y, position.z
                    ));
                }
            }
        }
        if !down {
            state.marker_drag = None;
        }
    }

    // Trace mode: a click on the terrain appends a key.
    if state.trace && !over_blocked && pointer.is_some() {
        if pressed {
            output.consumed = true;
        }
        if clicked
            && let Some(pointer) = pointer
            && let Some(hit) = pick_terrain(camera, rect, pointer, heightfield)
        {
            let add_to_target = state.trace_target;
            if add_to_target && rig.target.mode != TargetMode::Path {
                rig.target.mode = TargetMode::Path;
                rig.target.path.keys.clear();
            }
            let path = if add_to_target {
                &mut rig.target.path
            } else {
                &mut rig.eye
            };
            let last = path.keys.last().map_or(0.0, |key| key.time_s);
            let new_time = time.max(last + 1.0).clamp(0.0, 600.0);
            let position = if add_to_target {
                hit.to_array()
            } else {
                let altitude = f64::from(state.trace_altitude.max(0.0));
                [hit.x, hit.y, hit.z + altitude]
            };
            path.push(PathKey::new(new_time, position));
            rig.duration_s = rig.duration_s.max(new_time).clamp(0.1, 600.0);
            let index = path
                .keys
                .iter()
                .position(|key| (key.time_s - new_time).abs() < 1e-4 && key.position == position);
            if let Some(index) = index {
                state.selection = Some(if add_to_target {
                    AnimSelection::TargetKey(index)
                } else {
                    AnimSelection::EyeKey(index)
                });
            }
            output.changed = true;
            output.consumed = true;
            output.last_added_time = Some(new_time);
            output.status = Some(if add_to_target {
                format!("Cible ajoutée à t={new_time:.2}s")
            } else {
                format!("Point de vue ajouté à t={new_time:.2}s")
            });
        }
        if let Some(pointer) = pointer {
            let color = if state.trace_target {
                TARGET_COLOR
            } else {
                EYE_COLOR
            };
            // Show where the next key will land, including its altitude above
            // the picked ground, so tracing is not a blind click.
            if let Some(hit) = pick_terrain(camera, rect, pointer, heightfield) {
                let altitude = if state.trace_target {
                    0.0
                } else {
                    f64::from(state.trace_altitude.max(0.0))
                };
                let point = hit + DVec3::Z * altitude;
                if let Some(screen) = world_to_screen(camera, rect, point) {
                    if let Some(ground) = world_to_screen(camera, rect, hit) {
                        painter.line_segment(
                            [ground, screen],
                            Stroke::new(1.0, color.gamma_multiply(0.65)),
                        );
                        painter.circle_stroke(
                            ground,
                            3.0,
                            Stroke::new(1.0, color.gamma_multiply(0.55)),
                        );
                    }
                    painter.circle_filled(screen, 4.5, color.gamma_multiply(0.8));
                    painter.circle_stroke(screen, 7.0, Stroke::new(1.0, Color32::WHITE));
                    if altitude > 0.0 {
                        painter.text(
                            screen + Vec2::new(10.0, 6.0),
                            egui::Align2::LEFT_TOP,
                            format!("+{altitude:.0} m"),
                            egui::FontId::monospace(10.0),
                            color,
                        );
                    }
                }
            }
            painter.circle_stroke(pointer, 10.0, Stroke::new(1.2, color));
            painter.line_segment(
                [pointer - Vec2::X * 14.0, pointer + Vec2::X * 14.0],
                Stroke::new(1.0, color),
            );
            painter.line_segment(
                [pointer - Vec2::Y * 14.0, pointer + Vec2::Y * 14.0],
                Stroke::new(1.0, color),
            );
            painter.text(
                pointer + Vec2::new(16.0, -8.0),
                egui::Align2::LEFT_TOP,
                if state.trace_target {
                    "Cliquer : ajouter une cible"
                } else {
                    "Cliquer : ajouter un point de vue"
                },
                egui::FontId::proportional(11.0),
                color,
            );
        }
    }

    // Draw keys after interaction so the selection ring reflects this frame.
    if state.show_paths {
        draw_keys(
            &painter,
            rect,
            camera,
            rig,
            state.selection,
            state.hover_marker,
        );
    }

    if let Some(gizmo) = &gizmo {
        gizmo.draw(
            &painter,
            state.hover_axis,
            state.drag.as_ref().map(|d| d.axis),
        );
    }

    output
}

fn draw_path(
    painter: &egui::Painter,
    rect: Rect,
    camera: &Camera,
    path: &vf_core::Path3D,
    color: Color32,
    width: f32,
    _label: &str,
) {
    if path.keys.len() < 2 {
        return;
    }
    let duration = path.duration_s().max(0.1);
    let samples = 160;
    let mut previous: Option<Pos2> = None;
    for index in 0..=samples {
        let t = duration * index as f32 / samples as f32;
        let p = sample_path(path, t);
        let screen = world_to_screen(camera, rect, p);
        if let (Some(previous), Some(screen)) = (previous, screen) {
            painter.line_segment(
                [previous, screen],
                Stroke::new(width, color.gamma_multiply(0.85)),
            );
        }
        previous = screen;
    }
}

fn draw_trail(painter: &egui::Painter, rect: Rect, camera: &Camera, trail: &[[f64; 3]]) {
    let mut previous: Option<Pos2> = None;
    for point in trail {
        let screen = world_to_screen(camera, rect, DVec3::from_array(*point));
        if let (Some(previous), Some(screen)) = (previous, screen) {
            painter.line_segment(
                [previous, screen],
                Stroke::new(1.1, TRAIL_COLOR.gamma_multiply(0.65)),
            );
        }
        previous = screen;
    }
}

fn draw_comparison_trail(
    painter: &egui::Painter,
    rect: Rect,
    camera: &Camera,
    trail: &ComparisonTrail,
) {
    let color = preset_color(trail.preset).gamma_multiply(0.9);
    let mut previous: Option<Pos2> = None;
    for point in &trail.points {
        let screen = world_to_screen(camera, rect, DVec3::from_array(*point));
        if let (Some(previous), Some(screen)) = (previous, screen) {
            painter.line_segment([previous, screen], Stroke::new(1.9, color));
        }
        previous = screen;
    }
}

/// Materialise the current point of view in the design viewport: a wireframe
/// frustum from the eye toward its aim point. This is the "viewfinder" the
/// user is actually animating, and it is only drawn in design mode.
fn draw_camera_frustum(
    painter: &egui::Painter,
    rect: Rect,
    camera: &Camera,
    rig: &CameraRig,
    time: f32,
    live: Option<&RigSample>,
) {
    let (eye, target, up) = if let Some(sample) = live {
        if (sample.target - sample.camera.position).length_squared() < 1e-6 {
            return;
        }
        let mut up = sample.camera.up.normalize_or_zero();
        if up.length_squared() < 0.5 {
            up = DVec3::Z;
        }
        (sample.camera.position, sample.target, up)
    } else {
        let eye = sample_path(&rig.eye, time);
        let Some(target) = current_target(rig, eye, time) else {
            return;
        };
        let forward = (target - eye).normalize_or_zero();
        if forward.length_squared() < 0.5 {
            return;
        }
        let mut right = forward.cross(DVec3::Z).normalize_or_zero();
        if right.length_squared() < 0.5 {
            right = forward.cross(DVec3::X).normalize_or_zero();
        }
        if right.length_squared() < 0.5 {
            return;
        }
        let up = right.cross(forward).normalize_or_zero();
        (eye, target, up)
    };
    let forward = (target - eye).normalize_or_zero();
    let mut right = forward.cross(up).normalize_or_zero();
    if right.length_squared() < 0.5 {
        right = forward.cross(DVec3::X).normalize_or_zero();
    }
    let up = right.cross(forward).normalize_or_zero();
    let distance = ((target - eye).length() * 0.32).clamp(80.0, 2500.0);
    let aspect = f64::from((rect.width() / rect.height().max(1.0)).max(0.1));
    let half_h = distance * (f64::from(rig.optics.fov_y_deg) * 0.5).to_radians().tan();
    let half_w = half_h * aspect;
    let center = eye + forward * distance;
    let corners = [
        center - right * half_w - up * half_h,
        center + right * half_w - up * half_h,
        center + right * half_w + up * half_h,
        center - right * half_w + up * half_h,
    ];
    let projected: Vec<Option<Pos2>> = corners
        .iter()
        .map(|corner| world_to_screen(camera, rect, *corner))
        .collect();
    let eye_screen = world_to_screen(camera, rect, eye);
    let color = Color32::from_rgba_unmultiplied(120, 210, 255, 175);
    if let Some(eye_screen) = eye_screen {
        for corner in projected.iter().flatten() {
            painter.line_segment([eye_screen, *corner], Stroke::new(1.0, color));
        }
    }
    if let [Some(a), Some(b), Some(c), Some(d)] = projected[..] {
        painter.add(egui::Shape::convex_polygon(
            vec![a, b, c, d],
            Color32::from_rgba_unmultiplied(90, 180, 255, 22),
            Stroke::new(1.1, color),
        ));
    }
    for index in 0..4 {
        if let (Some(a), Some(b)) = (projected[index], projected[(index + 1) % 4]) {
            painter.line_segment([a, b], Stroke::new(1.1, color));
        }
    }
    if let Some(eye_screen) = eye_screen {
        painter.circle_stroke(eye_screen, 6.0, Stroke::new(0.9, color));
    }
}

fn draw_time_markers(
    painter: &egui::Painter,
    rect: Rect,
    camera: &Camera,
    rig: &CameraRig,
    time: f32,
    live: Option<&RigSample>,
) {
    let (eye, target) = if let Some(sample) = live {
        (sample.eye, Some(sample.target))
    } else {
        let eye = sample_path(&rig.eye, time);
        (eye, current_target(rig, eye, time))
    };
    let Some(eye_screen) = world_to_screen(camera, rect, eye) else {
        return;
    };
    painter.circle_filled(eye_screen, 5.0, EYE_COLOR);
    painter.circle_stroke(eye_screen, 7.5, Stroke::new(1.0, Color32::WHITE));
    if let Some(target) = target
        && let Some(target_screen) = world_to_screen(camera, rect, target)
    {
        painter.line_segment(
            [eye_screen, target_screen],
            Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 210, 140, 120)),
        );
        painter.circle_filled(target_screen, 4.0, TARGET_COLOR);
        painter.circle_stroke(target_screen, 8.0, Stroke::new(1.2, Color32::WHITE));
        painter.line_segment(
            [target_screen - Vec2::X * 4.0, target_screen + Vec2::X * 4.0],
            Stroke::new(1.0, TARGET_COLOR),
        );
        painter.line_segment(
            [target_screen - Vec2::Y * 4.0, target_screen + Vec2::Y * 4.0],
            Stroke::new(1.0, TARGET_COLOR),
        );
        painter.text(
            target_screen + Vec2::new(11.0, -10.0),
            egui::Align2::LEFT_TOP,
            "Cible",
            egui::FontId::proportional(10.0),
            TARGET_COLOR,
        );
    }
}

fn current_target(rig: &CameraRig, eye: DVec3, time: f32) -> Option<DVec3> {
    Some(match rig.target.mode {
        TargetMode::Fixed => DVec3::from_array(rig.target.fixed),
        TargetMode::Path => sample_path(&rig.target.path, time),
        TargetMode::EyeAhead => {
            let velocity = path_derivative(&rig.eye, time);
            eye + velocity * f64::from(rig.target.lead_s)
        }
        TargetMode::Manual => {
            let velocity = path_derivative(&rig.eye, time);
            if velocity.length_squared() > 0.01 {
                eye + velocity.normalize() * 500.0
            } else {
                eye + DVec3::new(0.0, 400.0, -100.0)
            }
        }
    })
}

fn draw_keys(
    painter: &egui::Painter,
    rect: Rect,
    camera: &Camera,
    rig: &CameraRig,
    selection: Option<AnimSelection>,
    hovered: Option<AnimSelection>,
) {
    for (index, key) in rig.eye.keys.iter().enumerate() {
        let selection_here = selection == Some(AnimSelection::EyeKey(index));
        let hovered_here = hovered == Some(AnimSelection::EyeKey(index));
        draw_key(
            painter,
            rect,
            camera,
            DVec3::from_array(key.position),
            EYE_COLOR,
            false,
            selection_here,
            hovered_here,
            &format!("E{index} · {:.1}s", key.time_s),
        );
    }
    if rig.target.mode == TargetMode::Path {
        for (index, key) in rig.target.path.keys.iter().enumerate() {
            let selection_here = selection == Some(AnimSelection::TargetKey(index));
            let hovered_here = hovered == Some(AnimSelection::TargetKey(index));
            draw_key(
                painter,
                rect,
                camera,
                DVec3::from_array(key.position),
                TARGET_COLOR,
                true,
                selection_here,
                hovered_here,
                &format!("T{index} · {:.1}s", key.time_s),
            );
        }
    }
    if rig.target.mode == TargetMode::Fixed {
        let selection_here = selection == Some(AnimSelection::FixedTarget);
        draw_key(
            painter,
            rect,
            camera,
            DVec3::from_array(rig.target.fixed),
            TARGET_COLOR,
            true,
            selection_here,
            false,
            "C",
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_key(
    painter: &egui::Painter,
    rect: Rect,
    camera: &Camera,
    position: DVec3,
    color: Color32,
    square: bool,
    selected: bool,
    hovered: bool,
    label: &str,
) {
    let Some(screen) = world_to_screen(camera, rect, position) else {
        return;
    };
    let radius = if selected {
        7.0
    } else if hovered {
        6.0
    } else {
        4.5
    };
    if square {
        let rect = Rect::from_center_size(screen, Vec2::splat(radius * 1.8));
        painter.rect_filled(rect, 2.0, color.gamma_multiply(0.7));
        painter.rect_stroke(
            rect,
            2.0,
            Stroke::new(1.0, if selected { Color32::WHITE } else { color }),
            egui::StrokeKind::Inside,
        );
    } else {
        painter.circle_filled(screen, radius, color.gamma_multiply(0.7));
        painter.circle_stroke(
            screen,
            radius + if selected { 2.5 } else { 0.5 },
            Stroke::new(if selected { 1.8 } else { 1.0 }, Color32::WHITE),
        );
    }
    if selected || hovered {
        painter.text(
            screen + Vec2::new(-9.0, 10.0),
            egui::Align2::RIGHT_TOP,
            label,
            egui::FontId::monospace(10.0),
            color,
        );
    }
}

fn selected_world_position(rig: &CameraRig, selection: Option<AnimSelection>) -> Option<DVec3> {
    match selection? {
        AnimSelection::EyeKey(index) => rig
            .eye
            .keys
            .get(index)
            .map(|key| DVec3::from_array(key.position)),
        AnimSelection::TargetKey(index) => rig
            .target
            .path
            .keys
            .get(index)
            .map(|key| DVec3::from_array(key.position)),
        AnimSelection::FixedTarget => Some(DVec3::from_array(rig.target.fixed)),
    }
}

fn selected_component(rig: &CameraRig, selection: Option<AnimSelection>, axis: Axis) -> f64 {
    let Some(position) = selected_world_position(rig, selection) else {
        return 0.0;
    };
    match axis {
        Axis::X => position.x,
        Axis::Y => position.y,
        Axis::Z => position.z,
    }
}

fn set_component(
    rig: &mut CameraRig,
    selection: Option<AnimSelection>,
    axis: Axis,
    value: f64,
) -> bool {
    let index = axis.index();
    let target = match selection {
        Some(AnimSelection::EyeKey(index)) => {
            rig.eye.keys.get_mut(index).map(|key| &mut key.position)
        }
        Some(AnimSelection::TargetKey(index)) if rig.target.mode == TargetMode::Path => rig
            .target
            .path
            .keys
            .get_mut(index)
            .map(|key| &mut key.position),
        Some(AnimSelection::FixedTarget) => Some(&mut rig.target.fixed),
        _ => None,
    };
    let Some(target) = target else {
        return false;
    };
    if !value.is_finite() || (target[index] - value).abs() < 1e-9 {
        return false;
    }
    target[index] = value;
    true
}

fn set_position(rig: &mut CameraRig, selection: Option<AnimSelection>, position: DVec3) -> bool {
    if position.to_array().iter().any(|value| !value.is_finite()) {
        return false;
    }
    let target = match selection {
        Some(AnimSelection::EyeKey(index)) => {
            rig.eye.keys.get_mut(index).map(|key| &mut key.position)
        }
        Some(AnimSelection::TargetKey(index)) if rig.target.mode == TargetMode::Path => rig
            .target
            .path
            .keys
            .get_mut(index)
            .map(|key| &mut key.position),
        Some(AnimSelection::FixedTarget) => Some(&mut rig.target.fixed),
        _ => None,
    };
    let Some(target) = target else {
        return false;
    };
    let next = position.to_array();
    if target.iter().zip(next).all(|(a, b)| (a - b).abs() < 1e-9) {
        return false;
    }
    *target = next;
    true
}

fn ray_plane(origin: DVec3, direction: DVec3, point: DVec3, normal: DVec3) -> Option<DVec3> {
    let denominator = direction.dot(normal);
    if denominator.abs() < 1e-6 {
        return None;
    }
    let t = (point - origin).dot(normal) / denominator;
    if t < 0.0 || !t.is_finite() {
        return None;
    }
    Some(origin + direction * t)
}

fn hit_marker(
    rect: Rect,
    camera: &Camera,
    rig: &CameraRig,
    pointer: Pos2,
) -> Option<AnimSelection> {
    let mut best: Option<(f32, AnimSelection)> = None;
    let mut consider = |position: DVec3, selection: AnimSelection| {
        if let Some(screen) = world_to_screen(camera, rect, position) {
            let distance = screen.distance(pointer);
            if distance <= 12.0 && best.is_none_or(|(d, _)| distance < d) {
                best = Some((distance, selection));
            }
        }
    };
    for (index, key) in rig.eye.keys.iter().enumerate() {
        consider(
            DVec3::from_array(key.position),
            AnimSelection::EyeKey(index),
        );
    }
    if rig.target.mode == TargetMode::Path {
        for (index, key) in rig.target.path.keys.iter().enumerate() {
            consider(
                DVec3::from_array(key.position),
                AnimSelection::TargetKey(index),
            );
        }
    }
    if rig.target.mode == TargetMode::Fixed {
        consider(
            DVec3::from_array(rig.target.fixed),
            AnimSelection::FixedTarget,
        );
    }
    best.map(|(_, selection)| selection)
}

struct Gizmo {
    origin: Pos2,
    endpoints: [Option<Pos2>; 3],
    world_length: f64,
}

impl Gizmo {
    fn build(rect: Rect, camera: &Camera, position: DVec3) -> Self {
        let distance = (position - camera.position).length().max(1.0);
        let tan_half = (f64::from(camera.fov_y_deg) * 0.5).to_radians().tan();
        let world_length = (distance * tan_half * 0.30).clamp(1.0, 50_000.0);
        let axes = [DVec3::X, DVec3::Y, DVec3::Z];
        let origin = world_to_screen(camera, rect, position).unwrap_or(rect.center());
        let endpoints =
            axes.map(|axis| world_to_screen(camera, rect, position + axis * world_length));
        Self {
            origin,
            endpoints,
            world_length,
        }
    }

    fn screen_dir(&self, axis: Axis) -> Vec2 {
        let Some(endpoint) = self.endpoints[axis.index()] else {
            return Vec2::X;
        };
        let dir = endpoint - self.origin;
        if dir.length_sq() < 1.0 {
            Vec2::X
        } else {
            dir.normalized()
        }
    }

    fn screen_length(&self, axis: Axis) -> f32 {
        self.endpoints[axis.index()]
            .map(|endpoint| (endpoint - self.origin).length())
            .unwrap_or(0.0)
    }

    fn hit_axis(&self, pointer: Pos2) -> Option<Axis> {
        let mut best: Option<(f32, Axis)> = None;
        for axis in [Axis::X, Axis::Y, Axis::Z] {
            let Some(endpoint) = self.endpoints[axis.index()] else {
                continue;
            };
            if self.screen_length(axis) < 8.0 {
                continue;
            }
            let distance = distance_to_segment(pointer, self.origin, endpoint);
            if distance <= 8.0 && best.is_none_or(|(d, _)| distance < d) {
                best = Some((distance, axis));
            }
        }
        best.map(|(_, axis)| axis)
    }

    fn draw(&self, painter: &egui::Painter, hover: Option<Axis>, dragging: Option<Axis>) {
        painter.circle_filled(self.origin, 4.0, Color32::WHITE);
        painter.circle_stroke(
            self.origin,
            6.0,
            Stroke::new(1.0, Color32::from_black_alpha(180)),
        );
        for axis in [Axis::X, Axis::Y, Axis::Z] {
            let Some(endpoint) = self.endpoints[axis.index()] else {
                continue;
            };
            if self.screen_length(axis) < 6.0 {
                continue;
            }
            let active = hover == Some(axis) || dragging == Some(axis);
            let color = AXIS_COLORS[axis.index()];
            let stroke = Stroke::new(if active { 3.0 } else { 2.0 }, color);
            painter.line_segment([self.origin, endpoint], stroke);
            let direction = (endpoint - self.origin).normalized();
            let tip = endpoint;
            let side = Vec2::new(-direction.y, direction.x);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    tip,
                    tip - direction * 10.0 + side * 5.0,
                    tip - direction * 10.0 - side * 5.0,
                ],
                color,
                Stroke::NONE,
            ));
            painter.text(
                tip + direction * 13.0,
                egui::Align2::CENTER_CENTER,
                AXIS_LABELS[axis.index()],
                egui::FontId::monospace(11.0),
                if active { Color32::WHITE } else { color },
            );
        }
    }
}

fn distance_to_segment(point: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let length_squared = ab.length_sq();
    if length_squared < 1e-6 {
        return point.distance(a);
    }
    let t = ((point - a).dot(ab) / length_squared).clamp(0.0, 1.0);
    point.distance(a + ab * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vf_core::Path3D;

    fn rig() -> CameraRig {
        CameraRig {
            eye: Path3D::default(),
            ..CameraRig::default()
        }
    }

    #[test]
    fn component_edits_both_paths_and_fixed_target() {
        let mut rig = rig();
        assert!(set_component(
            &mut rig,
            Some(AnimSelection::EyeKey(0)),
            Axis::Z,
            123.0
        ));
        assert!((rig.eye.keys[0].position[2] - 123.0).abs() < 1e-9);
        assert!(!set_component(
            &mut rig,
            Some(AnimSelection::EyeKey(0)),
            Axis::Z,
            123.0
        ));
        assert!(set_component(
            &mut rig,
            Some(AnimSelection::FixedTarget),
            Axis::X,
            42.0
        ));
        assert!((rig.target.fixed[0] - 42.0).abs() < 1e-9);
    }

    #[test]
    fn distance_to_segment_is_bounded() {
        let a = Pos2::new(0.0, 0.0);
        let b = Pos2::new(10.0, 0.0);
        assert!(distance_to_segment(Pos2::new(5.0, 3.0), a, b) > 2.9);
        assert!(distance_to_segment(Pos2::new(12.0, 0.0), a, b) < 2.1);
    }

    #[test]
    fn gizmo_hit_test_finds_a_translation_axis() {
        let camera = Camera {
            position: DVec3::new(0.0, 0.0, 500.0),
            target: DVec3::new(0.0, 0.0, 0.0),
            up: DVec3::Y,
            ..Camera::default()
        };
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let gizmo = Gizmo::build(rect, &camera, DVec3::ZERO);
        for axis in [Axis::X, Axis::Y] {
            let endpoint = gizmo.endpoints[axis.index()].expect("axis visible");
            let midpoint = gizmo.origin + (endpoint - gizmo.origin) * 0.5;
            assert_eq!(gizmo.hit_axis(midpoint), Some(axis));
        }
    }

    #[test]
    fn ray_plane_returns_the_intersection_in_front() {
        let hit = ray_plane(
            DVec3::new(0.0, 0.0, 100.0),
            DVec3::new(0.0, 0.0, -1.0),
            DVec3::new(10.0, 20.0, 50.0),
            DVec3::Z,
        )
        .unwrap();
        assert!((hit.x - 0.0).abs() < 1e-9);
        assert!((hit.z - 50.0).abs() < 1e-9);
        assert!(
            ray_plane(
                DVec3::new(0.0, 0.0, 100.0),
                DVec3::Z,
                DVec3::new(10.0, 20.0, 50.0),
                DVec3::Z,
            )
            .is_none()
        );
    }

    #[test]
    fn free_drag_moves_a_key_in_the_screen_plane() {
        let hf = HeightField::filled(65, 65, 10.0, 0.0).unwrap();
        let mut rig = rig();
        rig.enabled = true;
        rig.duration_s = 8.0;
        rig.eye.keys = vec![
            PathKey::new(0.0, [320.0, 320.0, 100.0]),
            PathKey::new(8.0, [320.0, 320.0, 140.0]),
        ];
        let camera = Camera {
            position: DVec3::new(320.0, 320.0, 800.0),
            target: DVec3::new(320.0, 320.0, 0.0),
            up: DVec3::Y,
            fov_y_deg: 50.0,
            ..Camera::default()
        };
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let center = rect.center();
        let ctx = egui::Context::default();
        let mut state = EditorState::new();
        let mut run = |events: Vec<egui::Event>, state: &mut EditorState| {
            let input = egui::RawInput {
                screen_rect: Some(rect),
                time: Some(1.0),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                show(
                    ui,
                    rect,
                    &camera,
                    &mut rig,
                    &hf,
                    0.0,
                    state,
                    None,
                    None,
                    None,
                    Rect::NOTHING,
                );
            });
            output.textures_delta.clear();
            output
        };
        run(
            vec![
                egui::Event::PointerMoved(center),
                egui::Event::PointerButton {
                    pos: center,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            &mut state,
        );
        assert_eq!(state.selection, Some(AnimSelection::EyeKey(0)));
        run(
            vec![egui::Event::PointerMoved(center + Vec2::new(90.0, -40.0))],
            &mut state,
        );
        run(
            vec![egui::Event::PointerButton {
                pos: center + Vec2::new(90.0, -40.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }],
            &mut state,
        );
        let moved = rig.eye.keys[0].position;
        assert!(
            (moved[0] - 320.0).abs() > 1.0 || (moved[1] - 320.0).abs() > 1.0,
            "screen-plane drag did not move the key: {moved:?}"
        );
    }

    #[test]
    fn trace_click_appends_a_key() {
        let hf = HeightField::filled(65, 65, 10.0, 0.0).unwrap();
        let mut rig = rig();
        rig.enabled = true;
        rig.duration_s = 8.0;
        rig.eye.keys = vec![
            PathKey::new(0.0, [320.0, 320.0, 100.0]),
            PathKey::new(8.0, [320.0, 320.0, 100.0]),
        ];
        let camera = Camera {
            position: DVec3::new(320.0, 320.0, 600.0),
            target: DVec3::new(320.0, 320.0, 0.0),
            up: DVec3::Y,
            fov_y_deg: 50.0,
            ..Camera::default()
        };
        let mut state = EditorState::new();
        state.trace = true;
        state.trace_altitude = 50.0;
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let center = rect.center();
        assert!(
            crate::animation_ui::pick_terrain(&camera, rect, center, &hf).is_some(),
            "the test ray must hit the flat heightfield"
        );
        let ctx = egui::Context::default();
        let run_editor =
            |rig: &mut CameraRig, state: &mut EditorState, events: Vec<egui::Event>| {
                let input = egui::RawInput {
                    screen_rect: Some(rect),
                    time: Some(1.0),
                    events,
                    ..Default::default()
                };
                let mut output = ctx.run_ui(input, |ui| {
                    show(
                        ui,
                        rect,
                        &camera,
                        rig,
                        &hf,
                        2.0,
                        state,
                        None,
                        None,
                        None,
                        Rect::NOTHING,
                    );
                });
                output.textures_delta.clear();
            };
        run_editor(
            &mut rig,
            &mut state,
            vec![
                egui::Event::PointerMoved(center),
                egui::Event::PointerButton {
                    pos: center,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                },
            ],
        );
        assert_eq!(rig.eye.keys.len(), 2, "press alone must not add a key");
        run_editor(
            &mut rig,
            &mut state,
            vec![egui::Event::PointerButton {
                pos: center,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            }],
        );
        assert_eq!(rig.eye.keys.len(), 3, "a click must append a traced key");
        let added = rig.eye.keys.last().unwrap();
        assert!(
            (added.position[2] - 50.0).abs() < 1.0,
            "{:?}",
            added.position
        );
    }
}
