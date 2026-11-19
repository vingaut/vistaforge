//! Timeline and graph editor for the camera rig.
//!
//! The animation workspace used to expose key times only as a list of DragValue
//! widgets. That makes retiming a shot guesswork. This dock draws the two
//! authored paths against a shared time ruler (timeline mode) and plots an
//! animation channel against time (graph mode). Keys can be selected, retimed
//! and, in graph mode, moved vertically. All edits go through the same
//! `CameraRig` as the numeric fields and the 3D gizmo, so the three editing
//! methods stay in sync.

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use vf_core::{CameraRig, PathKey, TargetMode};
use vf_flight::rig::{path_derivative, path_segments, sample_path};

use crate::animation_ui::AnimSelection;

const EYE_COLOR: Color32 = Color32::from_rgb(92, 202, 255);
const TARGET_COLOR: Color32 = Color32::from_rgb(255, 176, 92);
const GRID: Color32 = Color32::from_rgb(42, 54, 68);
const PANEL: Color32 = Color32::from_rgb(16, 21, 28);
const LABEL_WIDTH: f32 = 82.0;
const RULER_HEIGHT: f32 = 22.0;
const ROW_HEIGHT: f32 = 26.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Timeline,
    Graph,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Timeline => "Pistes",
            Self::Graph => "Graphe",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Channel {
    #[default]
    EyeX,
    EyeY,
    EyeZ,
    TargetX,
    TargetY,
    TargetZ,
    Speed,
    Bank,
}

impl Channel {
    pub const ALL: [Self; 8] = [
        Self::EyeX,
        Self::EyeY,
        Self::EyeZ,
        Self::TargetX,
        Self::TargetY,
        Self::TargetZ,
        Self::Speed,
        Self::Bank,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::EyeX => "Caméra X (m)",
            Self::EyeY => "Caméra Y (m)",
            Self::EyeZ => "Caméra Z (m)",
            Self::TargetX => "Cible X (m)",
            Self::TargetY => "Cible Y (m)",
            Self::TargetZ => "Cible Z (m)",
            Self::Speed => "Vitesse (m/s)",
            Self::Bank => "Roulis (°)",
        }
    }

    pub fn target(self) -> bool {
        matches!(self, Self::TargetX | Self::TargetY | Self::TargetZ)
    }

    pub fn axis(self) -> Option<usize> {
        match self {
            Self::EyeX | Self::TargetX => Some(0),
            Self::EyeY | Self::TargetY => Some(1),
            Self::EyeZ | Self::TargetZ => Some(2),
            Self::Speed | Self::Bank => None,
        }
    }

    pub fn read_only(self) -> bool {
        matches!(self, Self::Speed | Self::Bank)
    }
}

/// Simulated channels cached by the app. Eye/target channels are computed from
/// the authored curves directly and do not need this cache.
#[derive(Debug, Clone, Default)]
pub struct Curves {
    pub speed: Vec<(f32, f32)>,
    pub bank: Vec<(f32, f32)>,
}

#[derive(Debug, Clone)]
struct KeyDrag {
    target: bool,
    index: usize,
    start_pointer: Pos2,
    start_time: f32,
    start_value: f64,
    last_time: f32,
    last_position: [f64; 3],
    graph: bool,
    moved: bool,
}

#[derive(Debug, Clone)]
pub struct TimelineState {
    pub mode: Mode,
    pub channel: Channel,
    pub view_start: f32,
    pub view_end: f32,
    pub snap_s: f32,
    pub open: bool,
    drag: Option<KeyDrag>,
    scrub: bool,
}

impl Default for TimelineState {
    fn default() -> Self {
        Self {
            mode: Mode::Timeline,
            channel: Channel::EyeZ,
            view_start: 0.0,
            view_end: 0.0,
            snap_s: 0.0,
            open: true,
            drag: None,
            scrub: false,
        }
    }
}

impl TimelineState {
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }
}

pub struct TimelineInput<'a> {
    pub rig: &'a mut CameraRig,
    pub selection: &'a mut Option<AnimSelection>,
    pub time: &'a mut f32,
    pub edit_target: &'a mut bool,
    pub duration: f32,
    pub curves: Option<&'a Curves>,
    pub blocked: Rect,
}

#[derive(Debug, Default)]
pub struct TimelineOutput {
    pub changed: bool,
    pub seek: Option<f32>,
    pub status: Option<String>,
    pub insert: Option<(bool, f32)>,
    pub delete: bool,
}

pub fn show(
    ui: &mut egui::Ui,
    state: &mut TimelineState,
    input: TimelineInput<'_>,
) -> TimelineOutput {
    let mut output = TimelineOutput::default();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Timeline").strong());
        for mode in [Mode::Timeline, Mode::Graph] {
            if ui
                .selectable_label(state.mode == mode, mode.label())
                .clicked()
            {
                state.mode = mode;
            }
        }
        if state.mode == Mode::Graph {
            egui::ComboBox::from_id_salt("anim-graph-channel")
                .selected_text(state.channel.label())
                .show_ui(ui, |ui| {
                    for channel in Channel::ALL {
                        ui.selectable_value(&mut state.channel, channel, channel.label());
                    }
                });
        }
        ui.separator();
        ui.label("Aimant");
        ui.add(
            egui::DragValue::new(&mut state.snap_s)
                .speed(0.005)
                .range(0.0..=10.0)
                .suffix(" s"),
        );
        if ui.button("Tout voir").clicked() {
            state.view_start = 0.0;
            state.view_end = 0.0;
        }
        ui.label(
            egui::RichText::new(
                "Molette = zoom · glisser une clé = retiming · double-clic = insérer · clic droit = supprimer",
            )
            .small()
            .weak(),
        );
    });

    let block = input.blocked;
    let duration = input.duration.max(0.1);
    let fit_end = fit_end(input.rig, duration);
    if state.view_end <= state.view_start + 1e-3 {
        state.view_start = 0.0;
        state.view_end = fit_end.max(1.0);
    }
    // Clamp the zoom window; allow a little air beyond the last key.
    let hard_end = fit_end.max(1.0);
    state.view_start = state.view_start.clamp(0.0, (hard_end - 0.1).max(0.0));
    state.view_end = state
        .view_end
        .clamp(state.view_start + 0.1, hard_end.max(state.view_start + 0.1));

    let height = if state.mode == Mode::Timeline {
        RULER_HEIGHT + ROW_HEIGHT * 2.0 + 8.0
    } else {
        150.0
    };
    let height = height.min((ui.available_height() - 6.0).max(64.0));
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width().max(260.0), height),
        egui::Sense::click_and_drag(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, PANEL);
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0, Color32::from_rgb(50, 64, 80)),
        egui::StrokeKind::Inside,
    );

    let pointer = ui.input(|i| i.pointer.interact_pos());
    let pressed = ui.input(|i| i.pointer.primary_pressed());
    let down = ui.input(|i| i.pointer.primary_down());
    let released = ui.input(|i| i.pointer.primary_released());
    let secondary = ui.input(|i| i.pointer.secondary_clicked());
    let double = response.double_clicked();
    let over_blocked = block.is_positive() && block.contains(pointer.unwrap_or(Pos2::ZERO));

    // Wheel zoom around the pointer.
    if response.hovered() && !over_blocked {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.1 {
            let plot = plot_rect(rect, state.mode);
            let anchor = pointer.map_or(0.5, |p| {
                ((p.x - plot.left()) / plot.width().max(1.0)).clamp(0.0, 1.0)
            });
            let anchor_time = state.view_start + (state.view_end - state.view_start) * anchor;
            let factor = (-scroll * 0.002).exp();
            let span =
                ((state.view_end - state.view_start) * factor).clamp(0.2, fit_end.max(1.0) * 4.0);
            let mut start = anchor_time - span * anchor;
            let mut end = start + span;
            if start < 0.0 {
                end -= start;
                start = 0.0;
            }
            state.view_start = start;
            state.view_end = end;
        }
    }

    let mode = state.mode;
    let view_start = state.view_start;
    let view_end = state.view_end;
    let time_to_x = move |time: f32| -> f32 {
        let plot = plot_rect(rect, mode);
        let span = (view_end - view_start).max(1e-4);
        plot.left() + ((time - view_start) / span).clamp(0.0, 1.0) * plot.width()
    };
    let x_to_time = move |x: f32| -> f32 {
        let plot = plot_rect(rect, mode);
        let span = (view_end - view_start).max(1e-4);
        view_start + ((x - plot.left()) / plot.width().max(1.0)).clamp(0.0, 1.0) * span
    };

    match state.mode {
        Mode::Timeline => {
            draw_timeline(
                &painter,
                rect,
                state,
                input.rig,
                *input.selection,
                *input.time,
                duration,
                time_to_x,
            );
        }
        Mode::Graph => {
            draw_graph(
                &painter,
                rect,
                state,
                input.rig,
                input.curves,
                *input.selection,
                *input.time,
                time_to_x,
            );
        }
    }

    // Interaction.
    if pressed
        && !over_blocked
        && let Some(pointer) = pointer
    {
        if state.mode == Mode::Timeline {
            if let Some((target, index)) = hit_timeline_key(rect, state, input.rig, pointer) {
                begin_drag(
                    state,
                    input.rig,
                    *input.selection,
                    target,
                    index,
                    pointer,
                    false,
                );
            } else if rect.contains(pointer) {
                state.scrub = true;
                let time = x_to_time(pointer.x);
                output.seek = Some(time);
            }
        } else if let Some((target, index)) = hit_graph_key(rect, state, input.rig, pointer) {
            begin_drag(
                state,
                input.rig,
                *input.selection,
                target,
                index,
                pointer,
                true,
            );
        } else if graph_plot_rect(rect).contains(pointer) {
            state.scrub = true;
            output.seek = Some(x_to_time(pointer.x));
        }
    }

    let drag_graph_read_only = state.channel.read_only();
    let drag_graph_axis = state.channel.axis();
    let drag_snap = state.snap_s;
    let drag_view_span = (state.view_end - state.view_start).max(1e-4);
    let drag_plot_width = plot_rect(rect, state.mode).width().max(1.0);
    let drag_range = channel_range(input.rig, state.channel, state, input.curves);
    if down
        && let Some(drag) = state.drag.as_mut()
        && let Some(pointer) = pointer
    {
        {
            let dx = pointer.x - drag.start_pointer.x;
            let graph = graph_plot_rect(rect);
            let seconds_per_px = drag_view_span / drag_plot_width;
            let mut new_time = drag.start_time + dx * seconds_per_px;
            if drag_snap > 0.0 {
                new_time = (new_time / drag_snap).round() * drag_snap;
            }
            new_time = new_time.clamp(0.0, 600.0);
            let mut new_value = drag.start_value;
            if drag.graph && !drag_graph_read_only {
                let dy = pointer.y - drag.start_pointer.y;
                let value_span = (drag_range.1 - drag_range.0).max(1e-3);
                new_value = drag.start_value
                    - f64::from(dy) / f64::from(graph.height().max(1.0)) * value_span;
                if drag_snap > 0.0 {
                    new_value = (new_value / f64::from(drag_snap)).round() * f64::from(drag_snap);
                }
            }
            if (new_time - drag.last_time).abs() > 1e-6
                || (new_value - drag.start_value).abs() > 1e-6
            {
                if set_key_time(input.rig, drag.target, drag.index, new_time) {
                    drag.last_time = new_time;
                    drag.moved = true;
                    output.changed = true;
                }
                if drag.graph
                    && !drag_graph_read_only
                    && let Some(axis) = drag_graph_axis
                    && set_key_value(input.rig, drag.target, drag.index, axis, new_value)
                {
                    drag.last_position = key_position(input.rig, drag.target, drag.index)
                        .unwrap_or(drag.last_position);
                    drag.moved = true;
                    output.changed = true;
                }
                *input.selection = Some(if drag.target {
                    AnimSelection::TargetKey(drag.index)
                } else {
                    AnimSelection::EyeKey(drag.index)
                });
                *input.edit_target = drag.target;
            }
        }
    }

    if state.dragging_key()
        && (!down || released)
        && let Some(drag) = state.drag.take()
        && drag.moved
    {
        finish_key_edit(input.rig, input.selection, *input.edit_target, &drag);
        output.changed = true;
        output.status = Some(format!(
            "{} déplacée à t={:.2}s",
            if drag.target {
                "Clé cible"
            } else {
                "Clé caméra"
            },
            drag.last_time
        ));
    }
    if state.scrub {
        if down {
            if let Some(pointer) = pointer
                && !over_blocked
            {
                output.seek = Some(x_to_time(pointer.x));
            }
        } else {
            state.scrub = false;
        }
    }

    // Insert / delete.
    if double
        && !over_blocked
        && let Some(pointer) = pointer
    {
        if state.mode == Mode::Timeline
            && let Some((target, _)) = row_at(rect, state, input.rig, pointer)
        {
            let target = target && input.rig.target.mode == TargetMode::Path;
            output.insert = Some((target, x_to_time(pointer.x)));
        } else if state.mode == Mode::Graph && graph_plot_rect(rect).contains(pointer) {
            let target = state.channel.target() && input.rig.target.mode == TargetMode::Path
                || (!state.channel.target() && *input.edit_target);
            let target = target && input.rig.target.mode == TargetMode::Path;
            output.insert = Some((target, x_to_time(pointer.x)));
        }
    }
    if secondary
        && !over_blocked
        && let Some(pointer) = pointer
    {
        let hit = if state.mode == Mode::Timeline {
            hit_timeline_key(rect, state, input.rig, pointer)
        } else {
            hit_graph_key(rect, state, input.rig, pointer)
        };
        if let Some((target, index)) = hit {
            *input.selection = Some(if target {
                AnimSelection::TargetKey(index)
            } else {
                AnimSelection::EyeKey(index)
            });
            *input.edit_target = target;
            output.delete = true;
        }
    }

    // Playhead is drawn last so it stays above keys.
    let playhead = time_to_x(*input.time);
    let plot = plot_rect(rect, state.mode);
    if plot.x_range().contains(playhead) {
        let painter = ui.painter_at(rect);
        painter.line_segment(
            [
                Pos2::new(playhead, plot.top()),
                Pos2::new(playhead, plot.bottom()),
            ],
            Stroke::new(1.2, Color32::from_rgb(255, 235, 150)),
        );
        painter.circle_filled(
            Pos2::new(playhead, plot.top()),
            3.0,
            Color32::from_rgb(255, 235, 150),
        );
        painter.text(
            Pos2::new(playhead + 4.0, rect.top() + 3.0),
            egui::Align2::LEFT_TOP,
            format!("{:.2}s", input.time),
            egui::FontId::monospace(9.0),
            Color32::from_rgb(255, 235, 150),
        );
    }

    output
}

impl TimelineState {
    fn dragging_key(&self) -> bool {
        self.drag.is_some()
    }
}

fn begin_drag(
    state: &mut TimelineState,
    rig: &CameraRig,
    selection: Option<AnimSelection>,
    target: bool,
    index: usize,
    pointer: Pos2,
    graph: bool,
) {
    let Some(position) = key_position(rig, target, index) else {
        return;
    };
    let Some(time) = key_time(rig, target, index) else {
        return;
    };
    let axis = if graph {
        state.channel.axis().unwrap_or(2)
    } else {
        2
    };
    let start_value = position[axis];
    state.drag = Some(KeyDrag {
        target,
        index,
        start_pointer: pointer,
        start_time: time,
        start_value,
        last_time: time,
        last_position: position,
        graph,
        moved: false,
    });
    state.scrub = false;
    let _ = selection;
}

fn finish_key_edit(
    rig: &mut CameraRig,
    selection: &mut Option<AnimSelection>,
    edit_target: bool,
    drag: &KeyDrag,
) {
    if drag.target {
        rig.target
            .path
            .keys
            .sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
        if let Some(index) = rig.target.path.keys.iter().position(|key| {
            (key.time_s - drag.last_time).abs() < 1e-4 && key.position == drag.last_position
        }) {
            *selection = Some(AnimSelection::TargetKey(index));
        }
    } else {
        rig.eye.keys.sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
        if let Some(index) = rig.eye.keys.iter().position(|key| {
            (key.time_s - drag.last_time).abs() < 1e-4 && key.position == drag.last_position
        }) {
            *selection = Some(AnimSelection::EyeKey(index));
        }
    }
    let _ = edit_target;
}

fn key_time(rig: &CameraRig, target: bool, index: usize) -> Option<f32> {
    if target {
        rig.target.path.keys.get(index).map(|key| key.time_s)
    } else {
        rig.eye.keys.get(index).map(|key| key.time_s)
    }
}

fn key_position(rig: &CameraRig, target: bool, index: usize) -> Option<[f64; 3]> {
    if target {
        rig.target.path.keys.get(index).map(|key| key.position)
    } else {
        rig.eye.keys.get(index).map(|key| key.position)
    }
}

fn set_key_time(rig: &mut CameraRig, target: bool, index: usize, time: f32) -> bool {
    let key = if target {
        rig.target.path.keys.get_mut(index)
    } else {
        rig.eye.keys.get_mut(index)
    };
    let Some(key) = key else {
        return false;
    };
    if (key.time_s - time).abs() < 1e-6 {
        return false;
    }
    key.time_s = time;
    true
}

fn set_key_value(rig: &mut CameraRig, target: bool, index: usize, axis: usize, value: f64) -> bool {
    if axis > 2 || !value.is_finite() {
        return false;
    }
    let key = if target {
        rig.target.path.keys.get_mut(index)
    } else {
        rig.eye.keys.get_mut(index)
    };
    let Some(key) = key else {
        return false;
    };
    if (key.position[axis] - value).abs() < 1e-6 {
        return false;
    }
    key.position[axis] = value;
    true
}

fn fit_end(rig: &CameraRig, duration: f32) -> f32 {
    let mut end = duration.max(0.1);
    for key in &rig.eye.keys {
        end = end.max(key.time_s);
    }
    if rig.target.mode == TargetMode::Path {
        for key in &rig.target.path.keys {
            end = end.max(key.time_s);
        }
    }
    (end * 1.04).max(1.0)
}

fn plot_rect(rect: Rect, mode: Mode) -> Rect {
    match mode {
        Mode::Timeline => Rect::from_min_max(
            Pos2::new(rect.left() + LABEL_WIDTH, rect.top() + RULER_HEIGHT),
            Pos2::new(rect.right() - 4.0, rect.bottom() - 4.0),
        ),
        Mode::Graph => graph_plot_rect(rect),
    }
}

fn graph_plot_rect(rect: Rect) -> Rect {
    Rect::from_min_max(
        Pos2::new(rect.left() + 48.0, rect.top() + 18.0),
        Pos2::new(rect.right() - 8.0, rect.bottom() - 16.0),
    )
}

fn row_rects(rect: Rect, state: &TimelineState, rig: &CameraRig) -> [(bool, Rect); 2] {
    let top = rect.top() + RULER_HEIGHT;
    let plot = plot_rect(rect, state.mode);
    let eye = Rect::from_min_max(
        Pos2::new(plot.left(), top + 2.0),
        Pos2::new(plot.right(), top + ROW_HEIGHT - 2.0),
    );
    let target = Rect::from_min_max(
        Pos2::new(plot.left(), top + ROW_HEIGHT + 2.0),
        Pos2::new(plot.right(), top + ROW_HEIGHT * 2.0 - 2.0),
    );
    let _ = rig;
    [(false, eye), (true, target)]
}

fn row_at(
    rect: Rect,
    state: &TimelineState,
    rig: &CameraRig,
    pointer: Pos2,
) -> Option<(bool, usize)> {
    for (target, row) in row_rects(rect, state, rig) {
        if row.contains(pointer) {
            return Some((target, 0));
        }
    }
    None
}

fn hit_timeline_key(
    rect: Rect,
    state: &TimelineState,
    rig: &CameraRig,
    pointer: Pos2,
) -> Option<(bool, usize)> {
    let plot = plot_rect(rect, state.mode);
    let span = (state.view_end - state.view_start).max(1e-4);
    let mut best: Option<(f32, bool, usize)> = None;
    for (target, row) in row_rects(rect, state, rig) {
        if !row.contains(pointer) {
            continue;
        }
        let keys = if target {
            if rig.target.mode != TargetMode::Path {
                &[][..]
            } else {
                rig.target.path.keys.as_slice()
            }
        } else {
            rig.eye.keys.as_slice()
        };
        for (index, key) in keys.iter().enumerate() {
            let x = plot.left() + ((key.time_s - state.view_start) / span) * plot.width();
            let distance = (x - pointer.x).abs();
            if distance <= 8.0 && best.is_none_or(|(d, _, _)| distance < d) {
                best = Some((distance, target, index));
            }
        }
    }
    best.map(|(_, target, index)| (target, index))
}

fn hit_graph_key(
    rect: Rect,
    state: &TimelineState,
    rig: &CameraRig,
    pointer: Pos2,
) -> Option<(bool, usize)> {
    let axis = state.channel.axis()?;
    if state.channel.read_only() {
        return None;
    }
    let target = state.channel.target();
    let keys: &[PathKey] = if target {
        if rig.target.mode == TargetMode::Path {
            &rig.target.path.keys
        } else {
            &[]
        }
    } else {
        &rig.eye.keys
    };
    let plot = graph_plot_rect(rect);
    let (lo, hi) = channel_range(rig, state.channel, state, None);
    let span = (hi - lo).max(1e-6);
    let mut best: Option<(f32, bool, usize)> = None;
    for (index, key) in keys.iter().enumerate() {
        let x = plot.left()
            + ((key.time_s - state.view_start) / (state.view_end - state.view_start).max(1e-4))
                * plot.width();
        let y = plot.bottom()
            - ((key.position[axis] - lo) / span).clamp(0.0, 1.0) as f32 * plot.height();
        let distance = Pos2::new(x, y).distance(pointer);
        if distance <= 9.0 && best.is_none_or(|(d, _, _)| distance < d) {
            best = Some((distance, target, index));
        }
    }
    best.map(|(_, target, index)| (target, index))
}

#[allow(clippy::too_many_arguments)]
fn draw_timeline(
    painter: &egui::Painter,
    rect: Rect,
    state: &TimelineState,
    rig: &CameraRig,
    selection: Option<AnimSelection>,
    time: f32,
    duration: f32,
    time_to_x: impl Fn(f32) -> f32,
) {
    let plot = plot_rect(rect, state.mode);
    draw_time_grid(painter, rect, state, plot);
    for (target, row) in row_rects(rect, state, rig) {
        painter.rect_filled(
            row,
            3.0,
            if target {
                Color32::from_rgba_unmultiplied(64, 42, 20, 60)
            } else {
                Color32::from_rgba_unmultiplied(20, 48, 66, 60)
            },
        );
        let label = if target {
            match rig.target.mode {
                TargetMode::Path => "Cible",
                TargetMode::Fixed => "Cible fixe",
                TargetMode::EyeAhead => "Cible avance (courbe)",
                TargetMode::Offset => "Cible décalée",
                TargetMode::Trail => "Cible retard",
                TargetMode::Manual => "Cible manuelle",
                TargetMode::None => "Sans cible",
            }
        } else {
            "Caméra"
        };
        painter.text(
            Pos2::new(rect.left() + 8.0, row.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(11.0),
            if target { TARGET_COLOR } else { EYE_COLOR },
        );
    }

    // Segment bars: distance, duration and warning when the requested speed
    // exceeds the body limit. This is the timing information that the key
    // diamonds alone cannot show.
    for (target, row) in row_rects(rect, state, rig) {
        let path = if target {
            if rig.target.mode == TargetMode::Path {
                &rig.target.path
            } else {
                continue;
            }
        } else {
            &rig.eye
        };
        let segments = path_segments(path);
        for segment in &segments {
            let x0 = time_to_x(segment.start_time_s);
            let x1 = time_to_x(segment.end_time_s);
            let bar = Rect::from_min_max(
                Pos2::new(x0 + 2.0, row.top() + 5.0),
                Pos2::new((x1 - 2.0).max(x0 + 3.0), row.bottom() - 5.0),
            );
            if bar.width() < 4.0 {
                continue;
            }
            let too_fast = segment.average_speed_mps > rig.body.max_speed_mps + 0.5;
            let fill = if too_fast {
                Color32::from_rgba_unmultiplied(180, 78, 50, 90)
            } else if target {
                Color32::from_rgba_unmultiplied(90, 68, 36, 70)
            } else {
                Color32::from_rgba_unmultiplied(34, 72, 96, 70)
            };
            painter.rect_filled(bar, 3.0, fill);
            if bar.width() > 74.0 {
                painter.text(
                    bar.center(),
                    egui::Align2::CENTER_CENTER,
                    format!(
                        "{:.0} m · {:.1}s · {:.0} m/s",
                        segment.length_m,
                        segment.duration_s(),
                        segment.average_speed_mps
                    ),
                    egui::FontId::monospace(8.0),
                    if too_fast {
                        Color32::from_rgb(255, 180, 140)
                    } else {
                        Color32::from_rgb(175, 198, 215)
                    },
                );
            }
        }
    }

    // Duration end.
    let end_x = time_to_x(duration);
    if plot.x_range().contains(end_x) {
        painter.line_segment(
            [
                Pos2::new(end_x, plot.top()),
                Pos2::new(end_x, plot.bottom()),
            ],
            Stroke::new(1.0, Color32::from_rgb(120, 140, 160)),
        );
        painter.text(
            Pos2::new(end_x - 3.0, plot.top() + 2.0),
            egui::Align2::RIGHT_TOP,
            format!("fin {duration:.2}s"),
            egui::FontId::monospace(8.0),
            Color32::from_rgb(160, 176, 192),
        );
    }

    for (target, row) in row_rects(rect, state, rig) {
        let keys = if target {
            if rig.target.mode != TargetMode::Path {
                &[][..]
            } else {
                rig.target.path.keys.as_slice()
            }
        } else {
            rig.eye.keys.as_slice()
        };
        if target && rig.target.mode == TargetMode::Fixed {
            let x = time_to_x(0.0);
            draw_diamond(painter, Pos2::new(x, row.center().y), TARGET_COLOR, false);
            painter.text(
                Pos2::new(x + 8.0, row.center().y),
                egui::Align2::LEFT_CENTER,
                "C",
                egui::FontId::monospace(9.0),
                TARGET_COLOR,
            );
        }
        for (index, key) in keys.iter().enumerate() {
            let x = time_to_x(key.time_s);
            if !plot.x_range().contains(x) {
                continue;
            }
            let selected = selection
                == Some(if target {
                    AnimSelection::TargetKey(index)
                } else {
                    AnimSelection::EyeKey(index)
                });
            let color = if target { TARGET_COLOR } else { EYE_COLOR };
            draw_diamond(painter, Pos2::new(x, row.center().y), color, selected);
            if selected || (plot.width() / (keys.len().max(1) as f32) > 44.0) {
                painter.text(
                    Pos2::new(x, row.top() + 1.0),
                    egui::Align2::CENTER_BOTTOM,
                    format!(
                        "{}{} · {:.2}",
                        if target { "T" } else { "E" },
                        index,
                        key.time_s
                    ),
                    egui::FontId::monospace(8.5),
                    color,
                );
            }
        }
    }

    painter.text(
        Pos2::new(rect.left() + 8.0, rect.top() + 6.0),
        egui::Align2::LEFT_TOP,
        format!("{:.1}–{:.1}s", state.view_start, state.view_end),
        egui::FontId::monospace(9.0),
        Color32::from_rgb(150, 168, 186),
    );
    let _ = time;
}

#[allow(clippy::too_many_arguments)]
fn draw_graph(
    painter: &egui::Painter,
    rect: Rect,
    state: &TimelineState,
    rig: &CameraRig,
    curves: Option<&Curves>,
    selection: Option<AnimSelection>,
    _time: f32,
    time_to_x: impl Fn(f32) -> f32,
) {
    let plot = graph_plot_rect(rect);
    let (lo, hi) = channel_range(rig, state.channel, state, curves);
    let value_to_y = |value: f64| -> f32 {
        let span = (hi - lo).max(1e-6);
        plot.bottom() - ((value - lo) / span).clamp(0.0, 1.0) as f32 * plot.height()
    };

    // Background grid.
    let label_stride = if plot.height() < 100.0 { 2 } else { 1 };
    for step in 0..=4 {
        let y = plot.top() + plot.height() * step as f32 / 4.0;
        painter.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            Stroke::new(0.8, GRID),
        );
        if step % label_stride == 0 || step == 4 {
            let value = hi - (hi - lo) * f64::from(step) / 4.0;
            painter.text(
                Pos2::new(plot.left() - 5.0, y),
                egui::Align2::RIGHT_CENTER,
                format!("{value:.1}"),
                egui::FontId::monospace(8.5),
                Color32::from_rgb(150, 168, 186),
            );
        }
    }
    draw_time_grid(painter, rect, state, plot);

    // Curve.
    let samples = 192;
    let mut points = Vec::with_capacity(samples);
    for index in 0..=samples {
        let t =
            state.view_start + (state.view_end - state.view_start) * index as f32 / samples as f32;
        let value = channel_value(rig, state.channel, curves, t);
        if let Some(value) = value {
            points.push(Pos2::new(time_to_x(t), value_to_y(f64::from(value))));
        }
    }
    if points.len() >= 2 {
        painter.add(egui::Shape::line(
            points,
            Stroke::new(
                1.8,
                if state.channel.target() {
                    TARGET_COLOR
                } else {
                    EYE_COLOR
                },
            ),
        ));
    }

    // Key dots on editable channels.
    if let Some(axis) = state.channel.axis() {
        let target = state.channel.target();
        let keys: &[PathKey] = if target {
            if rig.target.mode == TargetMode::Path {
                &rig.target.path.keys
            } else {
                &[]
            }
        } else {
            &rig.eye.keys
        };
        for (index, key) in keys.iter().enumerate() {
            let selected = selection
                == Some(if target {
                    AnimSelection::TargetKey(index)
                } else {
                    AnimSelection::EyeKey(index)
                });
            let p = Pos2::new(time_to_x(key.time_s), value_to_y(key.position[axis]));
            if !plot.x_range().contains(p.x) {
                continue;
            }
            painter.circle_filled(p, if selected { 5.0 } else { 3.5 }, Color32::WHITE);
            painter.circle_stroke(
                p,
                if selected { 7.0 } else { 5.0 },
                Stroke::new(1.0, if target { TARGET_COLOR } else { EYE_COLOR }),
            );
            if selected {
                painter.text(
                    p + Vec2::new(8.0, -8.0),
                    egui::Align2::LEFT_TOP,
                    format!(
                        "{}{} · {:.2}s · {:.1}",
                        if target { "T" } else { "E" },
                        index,
                        key.time_s,
                        key.position[axis]
                    ),
                    egui::FontId::monospace(9.0),
                    Color32::WHITE,
                );
            }
        }
    }
    painter.text(
        Pos2::new(plot.left() + 4.0, plot.top() - 14.0),
        egui::Align2::LEFT_TOP,
        state.channel.label(),
        egui::FontId::proportional(10.0),
        Color32::from_rgb(190, 205, 220),
    );
}

fn draw_time_grid(painter: &egui::Painter, rect: Rect, state: &TimelineState, plot: Rect) {
    let span = (state.view_end - state.view_start).max(1e-4);
    let step = nice_step(span / 6.0);
    let mut t = (state.view_start / step).ceil() * step;
    while t <= state.view_end + 1e-4 {
        let x = plot.left() + ((t - state.view_start) / span) * plot.width();
        painter.line_segment(
            [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
            Stroke::new(0.6, GRID),
        );
        painter.text(
            Pos2::new(x + 2.0, rect.top() + 5.0),
            egui::Align2::LEFT_TOP,
            if step < 1.0 {
                format!("{t:.2}")
            } else {
                format!("{t:.0}")
            },
            egui::FontId::monospace(8.5),
            Color32::from_rgb(140, 158, 176),
        );
        t += step;
    }
}

fn nice_step(raw: f32) -> f32 {
    let raw = raw.max(1e-4);
    let magnitude = 10f32.powf(raw.log10().floor());
    let normalized = raw / magnitude;
    let factor = if normalized <= 1.0 {
        1.0
    } else if normalized <= 2.0 {
        2.0
    } else if normalized <= 5.0 {
        5.0
    } else {
        10.0
    };
    (factor * magnitude).max(1e-4)
}

fn draw_diamond(painter: &egui::Painter, center: Pos2, color: Color32, selected: bool) {
    let radius = if selected { 6.0 } else { 4.5 };
    painter.add(egui::Shape::convex_polygon(
        vec![
            center + Vec2::new(0.0, -radius),
            center + Vec2::new(radius, 0.0),
            center + Vec2::new(0.0, radius),
            center + Vec2::new(-radius, 0.0),
        ],
        color,
        Stroke::new(if selected { 1.5 } else { 1.0 }, Color32::WHITE),
    ));
}

fn channel_value(
    rig: &CameraRig,
    channel: Channel,
    curves: Option<&Curves>,
    time: f32,
) -> Option<f32> {
    match channel {
        Channel::EyeX | Channel::EyeY | Channel::EyeZ => {
            let point = sample_path(&rig.eye, time);
            Some(match channel {
                Channel::EyeX => point.x as f32,
                Channel::EyeY => point.y as f32,
                _ => point.z as f32,
            })
        }
        Channel::TargetX | Channel::TargetY | Channel::TargetZ => {
            let point = match rig.target.mode {
                TargetMode::Fixed => glam::DVec3::from_array(rig.target.fixed),
                TargetMode::Path => sample_path(&rig.target.path, time),
                TargetMode::EyeAhead
                | TargetMode::Offset
                | TargetMode::Trail
                | TargetMode::Manual
                | TargetMode::None => return None,
            };
            Some(match channel {
                Channel::TargetX => point.x as f32,
                Channel::TargetY => point.y as f32,
                _ => point.z as f32,
            })
        }
        Channel::Speed => curves
            .and_then(|curves| sample_curve(&curves.speed, time))
            .or_else(|| Some(path_derivative(&rig.eye, time).length() as f32)),
        Channel::Bank => curves.and_then(|curves| sample_curve(&curves.bank, time)),
    }
}

fn sample_curve(points: &[(f32, f32)], time: f32) -> Option<f32> {
    if points.is_empty() {
        return None;
    }
    let mut previous = points[0];
    for &point in points.iter().skip(1) {
        if time <= point.0 {
            let span = (point.0 - previous.0).max(1e-6);
            let f = ((time - previous.0) / span).clamp(0.0, 1.0);
            return Some(previous.1 + (point.1 - previous.1) * f);
        }
        previous = point;
    }
    points.last().map(|point| point.1)
}

fn channel_range(
    rig: &CameraRig,
    channel: Channel,
    state: &TimelineState,
    curves: Option<&Curves>,
) -> (f64, f64) {
    let mut values: Vec<f64> = Vec::new();
    let samples = 96;
    for index in 0..=samples {
        let t =
            state.view_start + (state.view_end - state.view_start) * index as f32 / samples as f32;
        if let Some(value) = channel_value(rig, channel, curves, t) {
            values.push(f64::from(value));
        }
    }
    if let Some(axis) = channel.axis() {
        let target = channel.target();
        let keys: &[PathKey] = if target {
            if rig.target.mode == TargetMode::Path {
                &rig.target.path.keys
            } else {
                &[]
            }
        } else {
            &rig.eye.keys
        };
        values.extend(keys.iter().map(|key| key.position[axis]));
    }
    let min = values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !min.is_finite() || !max.is_finite() {
        return (-1.0, 1.0);
    }
    let padding = ((max - min) * 0.08).max(0.5);
    (min - padding, max + padding)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_steps_are_round_numbers() {
        assert!((nice_step(0.03) - 0.05).abs() < 1e-6);
        assert!((nice_step(1.0) - 1.0).abs() < 1e-6);
        assert!((nice_step(7.0) - 10.0).abs() < 1e-6);
    }

    #[test]
    fn key_time_and_value_edits_are_applied() {
        let mut rig = CameraRig::default();
        rig.eye.keys = vec![
            PathKey::new(0.0, [0.0, 0.0, 10.0]),
            PathKey::new(8.0, [10.0, 0.0, 20.0]),
        ];
        assert!(set_key_time(&mut rig, false, 1, 5.0));
        assert!((rig.eye.keys[1].time_s - 5.0).abs() < 1e-6);
        assert!(set_key_value(&mut rig, false, 1, 2, 33.0));
        assert!((rig.eye.keys[1].position[2] - 33.0).abs() < 1e-6);
    }

    #[test]
    fn graph_channel_range_covers_keys() {
        let mut rig = CameraRig::default();
        rig.eye.keys = vec![
            PathKey::new(0.0, [0.0, 0.0, 10.0]),
            PathKey::new(8.0, [10.0, 0.0, 900.0]),
        ];
        let state = TimelineState {
            view_start: 0.0,
            view_end: 8.0,
            ..TimelineState::default()
        };
        let (lo, hi) = channel_range(&rig, Channel::EyeZ, &state, None);
        assert!(lo < 10.0 && hi > 900.0, "{lo}..{hi}");
    }
}
