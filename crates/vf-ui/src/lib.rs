//! Desktop interface for VistaForge.
//!
//! Layout: a toolbar (project/render), a control panel (terrain, scene, camera,
//! output) and an interactive viewport. Rendering happens on a worker thread so
//! the UI stays responsive; changes trigger a debounced low-resolution preview.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

mod animation_editor;
mod animation_ui;
mod file_picker;
mod gpu_view;
mod help;
mod history;
mod jobs;
mod timeline;
mod view_gizmo;

use eframe::egui;
use glam::DVec3;
use vf_core::{
    AircraftPreset, Camera, CameraLink, CameraRig, ClassicParams, ErosionParams, HeightField,
    LegacyMeta, ModifierKind, Palette, PathKey, Project, Quality, RenderMode, Seed, TargetMode,
    TerrainSource,
};
use vf_io::RgbaImage;
use vf_render::{RenderOptions, RenderOutput, render_with_options};
use vf_terrain::brush::{Brush, BrushMode, BrushShape, Falloff, Stroke, Symmetry};
use vf_terrain::{HillshadeParams, ResampleFilter};

const PREVIEW_SIZE: (u32, u32) = (960, 540);
/// Target duration of the quick interactive pass. The scale is learned from
/// the previous render so the preview adapts to the machine and the terrain.
const PREVIEW_TARGET_MS: f32 = 45.0;
const PREVIEW_FIRST_DELAY: Duration = Duration::from_millis(30);
const PREVIEW_SETTLE_DELAY: Duration = Duration::from_millis(200);
const PREVIEW_REFINE_DELAY: Duration = Duration::from_millis(180);
const MIN_PREVIEW_SCALE: f32 = 0.25;
const MAX_INTERACTIVE_SCALE: f32 = 0.8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Workspace {
    Terrain,
    Modifiers,
    Rock,
    Detail,
    Sculpt,
    Animation,
    Erosion,
    Water,
    Materials,
    Light,
    Camera,
    Render,
    Files,
    Help,
}

impl Workspace {
    const ALL: [Workspace; 14] = [
        Self::Terrain,
        Self::Modifiers,
        Self::Rock,
        Self::Detail,
        Self::Sculpt,
        Self::Animation,
        Self::Erosion,
        Self::Water,
        Self::Materials,
        Self::Light,
        Self::Camera,
        Self::Render,
        Self::Files,
        Self::Help,
    ];

    fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|workspace| workspace.label().to_ascii_lowercase() == normalized)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Terrain => "Terrain",
            Self::Modifiers => "Modificateurs",
            Self::Rock => "Roche",
            Self::Detail => "Détail",
            Self::Sculpt => "Sculpter",
            Self::Animation => "Animation",
            Self::Erosion => "Érosion",
            Self::Water => "Eau",
            Self::Materials => "Matières",
            Self::Light => "Lumière",
            Self::Camera => "Caméra",
            Self::Render => "Rendu",
            Self::Files => "Fichiers",
            Self::Help => "Aide",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerrainMode {
    Classic,
    Modern,
}

impl TerrainMode {
    fn label(self) -> &'static str {
        match self {
            Self::Classic => "Classic",
            Self::Modern => "Modern",
        }
    }
}

struct RenderTask {
    receiver: Receiver<std::result::Result<RenderOutput, String>>,
    started: Instant,
    full: bool,
    /// Project revision captured when the task was queued. A result whose
    /// revision no longer matches is shown only as a transient interactive
    /// preview (never applied as a final render).
    revision: u64,
    scale: f32,
}

enum AnimationExportMsg {
    Progress {
        done: usize,
        total: usize,
    },
    Finished {
        written: usize,
        total: usize,
        elapsed: Duration,
    },
    Cancelled,
    Failed(String),
}

struct HillshadeTask {
    receiver: Receiver<vf_terrain::GrayImage>,
    revision: u64,
}

struct GpuMeshTask {
    receiver: Receiver<std::result::Result<gpu_view::TerrainMeshData, String>>,
    revision: u64,
    started: Instant,
}

struct AnimationExportTask {
    receiver: Receiver<AnimationExportMsg>,
    cancel: Arc<AtomicBool>,
    started: Instant,
    total: usize,
    done: usize,
}

/// Offscreen documentation capture. Enabled with `VISTAFORGE_CAPTURE=DIR`:
/// the application walks a built-in tour, waits for each preview and writes a
/// PNG per step. It is a development tool; normal launches ignore it.
#[derive(Debug, Clone, Copy)]
struct CaptureStep {
    name: &'static str,
    workspace: Workspace,
    anim_tab: animation_ui::AnimTab,
    help: Option<help::HelpTopic>,
    design: bool,
    compare: bool,
    select_key: bool,
    preferences: bool,
}

struct CaptureState {
    dir: PathBuf,
    steps: Vec<CaptureStep>,
    index: usize,
    frame: u32,
    requested: bool,
    wait_after_request: u32,
}

impl CaptureState {
    fn new(dir: PathBuf) -> Self {
        use animation_ui::AnimTab;
        use help::HelpTopic;
        let mut steps = vec![
            CaptureStep {
                name: "animation_path",
                workspace: Workspace::Animation,
                anim_tab: AnimTab::Path,
                help: None,
                design: true,
                compare: false,
                select_key: true,
                preferences: false,
            },
            CaptureStep {
                name: "animation_help",
                workspace: Workspace::Animation,
                anim_tab: AnimTab::Path,
                help: Some(HelpTopic::Path),
                design: true,
                compare: false,
                select_key: true,
                preferences: false,
            },
            CaptureStep {
                name: "animation_target",
                workspace: Workspace::Animation,
                anim_tab: AnimTab::Path,
                help: Some(HelpTopic::Target),
                design: true,
                compare: false,
                select_key: false,
                preferences: false,
            },
            CaptureStep {
                name: "animation_profile",
                workspace: Workspace::Animation,
                anim_tab: AnimTab::Profile,
                help: Some(HelpTopic::Profile),
                design: true,
                compare: true,
                select_key: false,
                preferences: false,
            },
            CaptureStep {
                name: "animation_profile_clean",
                workspace: Workspace::Animation,
                anim_tab: AnimTab::Profile,
                help: None,
                design: true,
                compare: true,
                select_key: false,
                preferences: false,
            },
            CaptureStep {
                name: "animation_target_clean",
                workspace: Workspace::Animation,
                anim_tab: AnimTab::Path,
                help: None,
                design: true,
                compare: false,
                select_key: false,
                preferences: false,
            },
            CaptureStep {
                name: "animation_transport",
                workspace: Workspace::Animation,
                anim_tab: AnimTab::Preview,
                help: None,
                design: true,
                compare: false,
                select_key: false,
                preferences: false,
            },
            CaptureStep {
                name: "animation_timing",
                workspace: Workspace::Animation,
                anim_tab: AnimTab::Timing,
                help: Some(HelpTopic::Timing),
                design: true,
                compare: false,
                select_key: false,
                preferences: false,
            },
        ];
        for (name, workspace) in [
            ("workspace_terrain", Workspace::Terrain),
            ("workspace_world", Workspace::Terrain),
            ("workspace_modifiers", Workspace::Modifiers),
            ("workspace_rock", Workspace::Rock),
            ("workspace_detail", Workspace::Detail),
            ("workspace_sculpt", Workspace::Sculpt),
            ("workspace_erosion", Workspace::Erosion),
            ("workspace_water", Workspace::Water),
            ("workspace_materials", Workspace::Materials),
            ("workspace_light", Workspace::Light),
            ("workspace_camera", Workspace::Camera),
            ("workspace_render", Workspace::Render),
            ("workspace_files", Workspace::Files),
            ("workspace_help", Workspace::Help),
        ] {
            steps.push(CaptureStep {
                name,
                workspace,
                anim_tab: AnimTab::Path,
                help: None,
                design: true,
                compare: false,
                select_key: false,
                preferences: false,
            });
        }
        steps.push(CaptureStep {
            name: "workspace_compare",
            workspace: Workspace::Materials,
            anim_tab: AnimTab::Path,
            help: None,
            design: true,
            compare: false,
            select_key: false,
            preferences: false,
        });
        steps.push(CaptureStep {
            name: "workspace_preferences",
            workspace: Workspace::Help,
            anim_tab: AnimTab::Path,
            help: None,
            design: true,
            compare: false,
            select_key: false,
            preferences: true,
        });
        Self {
            dir,
            steps,
            index: 0,
            frame: 0,
            requested: false,
            wait_after_request: 0,
        }
    }
}

pub fn run(initial: Option<PathBuf>) -> eframe::Result<()> {
    // wgpu uses Vulkan on Linux/Windows and Metal on macOS. Set
    // VISTAFORGE_RENDERER=glow to fall back to the OpenGL UI renderer (and the
    // CPU-only preview) on machines where wgpu cannot initialise.
    let renderer = match std::env::var("VISTAFORGE_RENDERER") {
        Ok(value) if value.eq_ignore_ascii_case("glow") => eframe::Renderer::Glow,
        _ => eframe::Renderer::Wgpu,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1000.0, 640.0])
            .with_title("VistaForge"),
        renderer,
        ..Default::default()
    };
    eframe::run_native(
        "VistaForge",
        options,
        Box::new(move |cc| Ok(Box::new(VistaForgeApp::new(cc, initial)))),
    )
}

/// Preview quality requested from a panel. Draft/Normal/High stay at the
/// interactive preview size, Final renders at the project output resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewQuality {
    Draft,
    Normal,
    High,
    Final,
}

impl PreviewQuality {
    const ALL: [Self; 4] = [Self::Draft, Self::Normal, Self::High, Self::Final];

    fn label(self) -> &'static str {
        match self {
            Self::Draft => "Brouillon",
            Self::Normal => "Normal",
            Self::High => "Haute",
            Self::Final => "Finale",
        }
    }

    fn scale(self) -> f32 {
        match self {
            Self::Draft => 0.35,
            Self::Normal => 0.7,
            Self::High => 1.0,
            Self::Final => 1.0,
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            Self::Draft => "Aperçu 0,35× : très rapide pour régler un curseur.",
            Self::Normal => "Aperçu 0,7× : compromis vitesse / lisibilité.",
            Self::High => "Aperçu 1× (960×540) : détail et matières lisibles.",
            Self::Final => "Rendu à la résolution de sortie du projet (plus lent).",
        }
    }
}

/// Before/after comparison of the current panel edits.
///
/// A CPU preview is the only renderer that shows the continuous detail and
/// the material pass, so compare mode forces it even when the GPU viewport is
/// available. The left side is a frozen snapshot taken when compare was
/// enabled; the right side follows the live project. Apply keeps the result,
/// Revert restores the snapshot (project + heightfield).
/// A small 2D paint canvas used to author a custom rock texture without
/// leaving VistaForge. The pixels are saved to a PNG and then referenced by the
/// material's texture path, so the normal bump/displacement pipeline applies.
struct PaintState {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
    texture: Option<egui::TextureHandle>,
    dirty: bool,
    open: bool,
    brush_radius: f32,
    color: [u8; 3],
    strength: f32,
    eraser: bool,
    last: Option<(usize, usize)>,
    path: String,
    status: String,
}

impl PaintState {
    fn new() -> Self {
        Self {
            width: 256,
            height: 256,
            pixels: vec![118; 256 * 256 * 4],
            texture: None,
            dirty: true,
            open: false,
            brush_radius: 18.0,
            color: [132, 118, 98],
            strength: 0.65,
            eraser: false,
            last: None,
            path: "painted_rock.png".into(),
            status: "Pinceau prêt".into(),
        }
    }

    fn fill_procedural(&mut self) {
        use vf_core::Seed;
        let noise = vf_procgen::Noise::new(Seed::new(0x5EED_C0DE));
        for y in 0..self.height {
            for x in 0..self.width {
                let u = x as f32 / self.width as f32;
                let v = y as f32 / self.height as f32;
                let coarse = noise.fbm01(u * 3.0, v * 3.0, 4, 2.0, 0.5);
                let fine = noise.fbm01(u * 18.0 + 11.0, v * 18.0 - 7.0, 3, 2.0, 0.5);
                let value = (0.42 + coarse * 0.34 + fine * 0.16).clamp(0.0, 1.0);
                let rgb = [
                    (value * 196.0 + 26.0) as u8,
                    (value * 172.0 + 22.0) as u8,
                    (value * 146.0 + 20.0) as u8,
                ];
                let index = (y * self.width + x) * 4;
                self.pixels[index] = rgb[0];
                self.pixels[index + 1] = rgb[1];
                self.pixels[index + 2] = rgb[2];
                self.pixels[index + 3] = 255;
            }
        }
        self.dirty = true;
        self.status = "Bruit de roche généré".into();
    }

    fn paint(&mut self, x: usize, y: usize) {
        let radius = self.brush_radius.max(1.0) as i32;
        let r2 = (radius * radius) as f32;
        let strength = self.strength.clamp(0.0, 1.0);
        for dy in -radius..=radius {
            let py = y as i32 + dy;
            if py < 0 || py >= self.height as i32 {
                continue;
            }
            for dx in -radius..=radius {
                let px = x as i32 + dx;
                if px < 0 || px >= self.width as i32 {
                    continue;
                }
                let d2 = (dx * dx + dy * dy) as f32;
                if d2 > r2 {
                    continue;
                }
                let falloff = 1.0 - (d2 / r2).sqrt();
                let alpha = (falloff * strength).clamp(0.0, 1.0);
                let index = (py as usize * self.width + px as usize) * 4;
                if self.eraser {
                    // Erase toward the neutral mid-grey rather than black, so
                    // the displacement map keeps a sensible baseline.
                    for channel in 0..3 {
                        let value = self.pixels[index + channel] as f32;
                        self.pixels[index + channel] =
                            (value + (128.0 - value) * alpha).clamp(0.0, 255.0) as u8;
                    }
                } else {
                    for channel in 0..3 {
                        let value = self.pixels[index + channel] as f32;
                        let target = self.color[channel] as f32;
                        self.pixels[index + channel] =
                            (value + (target - value) * alpha).clamp(0.0, 255.0) as u8;
                    }
                }
                self.pixels[index + 3] = 255;
            }
        }
        self.dirty = true;
    }

    fn paint_line(&mut self, from: (usize, usize), to: (usize, usize)) {
        let dx = to.0 as i64 - from.0 as i64;
        let dy = to.1 as i64 - from.1 as i64;
        let steps = dx.abs().max(dy.abs()).max(1);
        for step in 0..=steps {
            let x = from.0 as i64 + dx * step / steps;
            let y = from.1 as i64 + dy * step / steps;
            self.paint(x.max(0) as usize, y.max(0) as usize);
        }
    }

    fn save(&mut self, project_path: Option<&Path>) -> Result<String, String> {
        let path = match project_path {
            Some(base) if !Path::new(&self.path).is_absolute() => base
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(&self.path),
            _ => PathBuf::from(&self.path),
        };
        let buffer =
            image::RgbaImage::from_raw(self.width as u32, self.height as u32, self.pixels.clone())
                .ok_or_else(|| "dimensions de peinture invalides".to_string())?;
        image::DynamicImage::ImageRgba8(buffer)
            .save(&path)
            .map_err(|error| format!("écriture {path:?} : {error}"))?;
        self.path = path.display().to_string();
        self.status = format!("Enregistré {}", path.display());
        Ok(self.path.clone())
    }
}

#[derive(Default)]
struct CompareState {
    enabled: bool,
    /// Normalised split position, 0 = all before, 1 = all after.
    split: f32,
    before: Option<egui::TextureHandle>,
    baseline_project: Option<Project>,
    baseline_heightfield: Option<Arc<HeightField>>,
    /// The next completed CPU render becomes the baseline.
    waiting_for_baseline: bool,
    dragging: bool,
}

struct VistaForgeApp {
    project: Project,
    /// Copy-on-write terrain. Preview renders, background jobs and undo
    /// snapshots share the same samples until an edit actually needs a new
    /// version, which keeps large heightfields responsive.
    heightfield: Arc<HeightField>,
    /// Cached heightfield statistics. `workspace_panel` is redrawn every
    /// frame; scanning a 4k/16k heightfield there would freeze the UI.
    terrain_stats: Option<vf_core::TerrainStats>,
    terrain_stats_hf: Option<Arc<HeightField>>,
    project_path: Option<PathBuf>,
    path_input: String,
    render_path: String,
    status: String,

    texture: Option<egui::TextureHandle>,
    last_image: Option<RgbaImage>,
    render_task: Option<RenderTask>,
    last_render_ms: u128,
    last_render_size: (u32, u32),
    dirty_since: Option<Instant>,
    dirty_first: Option<Instant>,
    preview_scale: f32,
    interactive_scale: f32,
    adaptive_preview: bool,
    refine_at: Option<Instant>,
    preview_ms: u128,
    revision: u64,
    pending_full: bool,
    compare: CompareState,
    paint: PaintState,
    preview_quality: PreviewQuality,
    /// Keep the current orbit pose when the terrain is regenerated, imported
    /// or composed. The previous behaviour auto-framed and lost a detail view
    /// the user had carefully positioned.
    preserve_view: bool,

    gpu: Option<gpu_view::GpuViewport>,
    gpu_enabled: bool,
    gpu_mesh_dirty: bool,
    gpu_mesh_due: Option<Instant>,
    gpu_mesh_revision: u64,
    gpu_mesh_task: Option<GpuMeshTask>,
    gpu_error: Option<String>,
    show_final_render: bool,
    /// Force the CPU preview even when a GPU viewport is available. This is
    /// the only view that shows continuous detail, material bump and custom
    /// textures; the GPU viewport is a fast navigation preview.
    cpu_preview_forced: bool,

    workspace: Workspace,
    mode: TerrainMode,
    size: u32,
    seed: u64,
    preset: String,
    rock_preset: String,
    dimension: f32,
    divisor: u8,
    island: bool,
    vertical_scale: f32,

    yaw: f32,
    pitch: f32,
    distance: f64,
    orbit_center: DVec3,
    fov: f32,
    show_gizmo: bool,

    render_width: u32,
    render_height: u32,
    quality: Quality,
    shadows: bool,
    ao: bool,
    clouds: bool,
    water: bool,
    exposure: f32,
    selected_modifier: usize,
    add_modifier_kind: ModifierKind,

    sculpt_brush: Brush,
    sculpt_stroke: Option<Stroke>,
    sculpt_texture: Option<egui::TextureHandle>,
    sculpt_texture_dirty: bool,
    hillshade_revision: u64,
    hillshade_task: Option<HillshadeTask>,
    sculpt_detail: u32,
    sculpt_exaggeration: f32,
    sculpt_contours_m: f32,
    sculpt_zoom: f32,
    sculpt_pan: egui::Vec2,
    sculpt_cursor: Option<(f64, f64)>,
    sculpt_status: String,

    rig_time: f32,
    rig_status: String,

    anim_tab: animation_ui::AnimTab,
    anim_view: animation_ui::AnimViewMode,
    anim_design_camera: Option<Camera>,
    anim_editor: animation_editor::EditorState,
    anim_edit_target: bool,
    anim_camera_dirty: bool,
    anim_profile_cache: Option<animation_ui::ProfileCache>,
    anim_trail_cache: Option<(u64, Vec<[f64; 3]>)>,
    anim_compare: bool,
    anim_compare_cache: Option<(u64, Vec<animation_ui::ComparisonTrail>)>,
    anim_transport_rect: egui::Rect,
    anim_framed: bool,
    anim_revision: u64,
    anim_playing: bool,
    anim_loop: bool,
    anim_play_speed: f32,
    anim_last_play_preview: Instant,
    anim_track: Option<(u64, vf_flight::rig::RigTrack)>,
    anim_track_error: Option<f32>,
    anim_live_sample: Option<vf_flight::rig::RigSample>,
    anim_timeline: timeline::TimelineState,
    anim_curve_cache: Option<(u64, timeline::Curves)>,
    anim_io_path: String,
    anim_stretch_keys: bool,
    anim_target_speed: f32,

    files_status: String,
    files_import_path: String,
    files_export_path: String,
    files_min_height: f32,
    files_max_height: f32,
    files_spacing: f32,
    files_flip_y: bool,
    files_invert_height: bool,
    files_nodata: f32,
    blender_export_dir: String,
    blender_subdivision: u32,
    blender_export_animation: bool,
    file_picker: file_picker::FilePicker,
    anim_export_dir: String,
    anim_export_task: Option<AnimationExportTask>,
    task: Option<jobs::Task>,

    history: history::History,
    preferences_open: bool,
    help: help::HelpState,
    last_viewport_rect: egui::Rect,
    capture: Option<CaptureState>,
    /// Capture-only: 0 = baseline requested, 1 = material changed for the
    /// before/after screenshot.
    capture_compare_stage: u8,
}

impl VistaForgeApp {
    fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        let mut status = String::new();
        let (project, heightfield, project_path) = if let Some(path) = initial {
            match vf_io::load_project(&path) {
                Ok(loaded) => {
                    status = format!("Loaded {}", path.display());
                    (loaded.project, loaded.heightfield, Some(path))
                }
                Err(error) => {
                    status = format!("Could not load {}: {error}", path.display());
                    let (project, heightfield) = default_scene();
                    (project, heightfield, None)
                }
            }
        } else {
            let (project, heightfield) = default_scene();
            (project, heightfield, None)
        };
        let heightfield = Arc::new(heightfield);
        let history = history::History::new(
            history::load_settings().unwrap_or_default(),
            &project,
            &heightfield,
        );
        let mut app = Self {
            project,
            heightfield,
            terrain_stats: None,
            terrain_stats_hf: None,
            history,
            preferences_open: false,
            project_path: project_path.clone(),
            path_input: project_path
                .as_ref()
                .map_or_else(|| "landscape.vfproj".into(), |p| p.display().to_string()),
            render_path: "render.png".into(),
            status,
            texture: None,
            last_image: None,
            render_task: None,
            last_render_ms: 0,
            last_render_size: (0, 0),
            dirty_since: None,
            dirty_first: None,
            preview_scale: 1.0,
            interactive_scale: 0.5,
            adaptive_preview: true,
            refine_at: None,
            preview_ms: 0,
            revision: 0,
            pending_full: false,
            compare: CompareState {
                split: 0.5,
                ..CompareState::default()
            },
            paint: PaintState::new(),
            preview_quality: PreviewQuality::Normal,
            preserve_view: true,
            gpu: None,
            gpu_enabled: true,
            gpu_mesh_dirty: false,
            gpu_mesh_due: None,
            gpu_mesh_revision: 0,
            gpu_mesh_task: None,
            gpu_error: None,
            show_final_render: false,
            cpu_preview_forced: false,
            workspace: Workspace::Terrain,
            mode: TerrainMode::Classic,
            size: 258,
            seed: 1,
            preset: "alpine".into(),
            rock_preset: "none".into(),
            dimension: 100.0,
            divisor: 1,
            island: false,
            vertical_scale: 1400.0,
            yaw: 35.0,
            pitch: 25.0,
            distance: 5000.0,
            orbit_center: DVec3::ZERO,
            fov: 48.0,
            show_gizmo: true,
            render_width: 1280,
            render_height: 720,
            quality: Quality::High,
            shadows: true,
            ao: false,
            clouds: true,
            water: true,
            exposure: 1.0,
            selected_modifier: 0,
            add_modifier_kind: ModifierKind::Displace,
            sculpt_brush: Brush::default(),
            sculpt_stroke: None,
            sculpt_texture: None,
            sculpt_texture_dirty: true,
            hillshade_revision: 0,
            hillshade_task: None,
            sculpt_detail: 2,
            sculpt_exaggeration: 1.8,
            sculpt_contours_m: 5.0,
            sculpt_zoom: 1.0,
            sculpt_pan: egui::Vec2::ZERO,
            sculpt_cursor: None,
            sculpt_status: "Prêt. Clic gauche = sculpter, clic droit = déplacer.".into(),
            rig_time: 0.0,
            rig_status: "Tracez le point de vue, puis réglez la cible et le profil de vol.".into(),
            anim_tab: animation_ui::AnimTab::Path,
            anim_view: animation_ui::AnimViewMode::Design,
            anim_design_camera: None,
            anim_editor: animation_editor::EditorState::new(),
            anim_edit_target: false,
            anim_camera_dirty: false,
            anim_profile_cache: None,
            anim_trail_cache: None,
            anim_compare: false,
            anim_compare_cache: None,
            anim_transport_rect: egui::Rect::NOTHING,
            anim_framed: false,
            anim_revision: 0,
            anim_playing: false,
            anim_loop: true,
            anim_play_speed: 1.0,
            anim_last_play_preview: Instant::now(),
            anim_track: None,
            anim_track_error: None,
            anim_live_sample: None,
            anim_timeline: timeline::TimelineState::default(),
            anim_curve_cache: None,
            anim_io_path: "anim-out/flyover.glb".into(),
            anim_stretch_keys: true,
            anim_target_speed: 0.0,
            files_status:
                "Import/export GeoTIFF, ASCII, DEM, heightmap PNG/JPEG, Blender GLB et animations."
                    .into(),
            files_import_path: "import/dem.tif".into(),
            files_export_path: "export/dem.tif".into(),
            files_min_height: 0.0,
            files_max_height: 0.0,
            files_spacing: 0.0,
            files_flip_y: true,
            files_invert_height: false,
            files_nodata: -9999.0,
            blender_export_dir: "blender-out".into(),
            blender_subdivision: 2,
            blender_export_animation: true,
            file_picker: file_picker::FilePicker::default(),
            anim_export_dir: "anim-out".into(),
            anim_export_task: None,
            task: None,
            help: help::HelpState::new(),
            last_viewport_rect: egui::Rect::NOTHING,
            capture_compare_stage: 0,
            capture: std::env::var("VISTAFORGE_CAPTURE")
                .ok()
                .filter(|dir| !dir.trim().is_empty())
                .map(|dir| CaptureState::new(PathBuf::from(dir))),
        };
        app.orbit_center = vf_scene::terrain_center(&app.heightfield);
        app.sync_from_project();
        app.sync_orbit_from_camera();
        if let Some(state) = cc.wgpu_render_state.as_ref() {
            match gpu_view::GpuViewport::new(state, &app.project, &app.heightfield, app.water) {
                Ok(gpu) => {
                    app.status = format!("Vue 3D GPU : {}", gpu.label());
                    app.gpu = Some(gpu);
                }
                Err(error) => {
                    app.gpu_error = Some(error.clone());
                    app.status = format!("GPU indisponible : {error}");
                }
            }
        }
        if let Some(workspace) = std::env::var("VISTAFORGE_WORKSPACE")
            .ok()
            .and_then(|value| Workspace::parse(&value))
        {
            app.workspace = workspace;
        }
        if let Some(tab) = std::env::var("VISTAFORGE_ANIM_TAB")
            .ok()
            .and_then(|value| animation_ui::AnimTab::parse(&value))
        {
            app.anim_tab = tab;
        }
        if std::env::var("VISTAFORGE_TIMELINE_GRAPH").is_ok() {
            app.anim_timeline.mode = timeline::Mode::Graph;
            app.anim_timeline.channel = timeline::Channel::Bank;
        }
        if app.workspace == Workspace::Animation {
            app.ensure_animation_workspace(&cc.egui_ctx);
        }
        if let Some(topic) = std::env::var("VISTAFORGE_HELP_TOPIC")
            .ok()
            .and_then(|value| help::HelpTopic::parse(&value))
        {
            app.help.reveal(topic);
        }
        if app.capture.is_some() {
            // Documentation captures need the full preview, not the
            // interaction-scale image.
            app.adaptive_preview = false;
            app.preview_scale = 1.0;
        }
        if app.gpu_active() {
            // The GPU viewport is live; no CPU preview is needed at startup.
            app.gpu_mesh_dirty = false;
            app.gpu_mesh_due = None;
        } else {
            app.preview_scale = if app.adaptive_preview {
                app.interactive_scale
            } else {
                1.0
            };
            app.start_render(cc.egui_ctx.clone(), false);
        }
        app
    }

    fn sync_from_project(&mut self) {
        self.anim_framed = false;
        self.orbit_center = vf_scene::terrain_center(&self.heightfield);
        match &self.project.source {
            TerrainSource::Classic { params } => {
                self.mode = TerrainMode::Classic;
                self.size = self.heightfield.width() as u32;
                self.seed = params.seed;
                self.dimension = params.fractal_dimension;
                self.divisor = params.divisor;
                self.island = params.island;
                self.vertical_scale = params.vertical_scale;
            }
            TerrainSource::Modern { params } => {
                self.mode = TerrainMode::Modern;
                self.size = self.heightfield.width() as u32;
                self.seed = params.seed;
                self.preset = params.preset.clone();
                self.vertical_scale = params.vertical_scale;
            }
            TerrainSource::Dem { .. } => {
                self.size = self.heightfield.width() as u32;
                self.seed = self.project.seed.0;
            }
        }
        self.rock_preset = self.project.geology.preset.clone();
        self.fov = self.project.camera.fov_y_deg;
        self.quality = self.project.render.quality;
        self.shadows = self.project.render.shadows;
        self.ao = self.project.render.ao;
        self.clouds = self.project.render.clouds;
        self.water = self.project.render.water;
        self.exposure = self.project.render.exposure;
        self.render_width = self.project.render.width.clamp(64, 8192);
        self.render_height = self.project.render.height.clamp(64, 8192);
        self.selected_modifier = self
            .selected_modifier
            .min(self.project.modifiers.items.len());
    }

    fn sync_orbit_from_camera(&mut self) {
        let center = self.orbit_center;
        let delta = self.project.camera.position - center;
        let horizontal = (delta.x * delta.x + delta.y * delta.y).sqrt();
        self.distance = delta.length().max(1.0);
        self.pitch = (delta.z / self.distance)
            .clamp(-1.0, 1.0)
            .asin()
            .to_degrees() as f32;
        // At the poles the yaw is undefined; keep the previous value so the
        // top/bottom presets do not cause a jump when the user resumes orbiting.
        if horizontal > 1e-6 {
            self.yaw = (delta.x.atan2(-delta.y).to_degrees()) as f32;
        }
        self.rock_preset = self.project.geology.preset.clone();
        self.fov = self.project.camera.fov_y_deg;
    }

    fn apply_orbit(&mut self) {
        self.project.camera = vf_scene::orbit(
            self.orbit_center,
            self.distance,
            self.yaw,
            self.pitch,
            self.fov,
        );
    }

    fn auto_frame(&mut self) {
        self.orbit_center = vf_scene::terrain_center(&self.heightfield);
        let aspect = f64::from(self.render_width.max(1)) / f64::from(self.render_height.max(1));
        self.project.camera = vf_scene::auto_frame_with_fov(&self.heightfield, aspect, self.fov);
        self.sync_orbit_from_camera();
    }

    /// Keep the user's viewing angle and distance after a terrain swap. Only
    /// the orbit pivot moves to the new terrain centre; yaw, pitch, distance
    /// and FOV are preserved. Auto-frame is still available with the checkbox
    /// off or the "Recadrer" button.
    fn adopt_view_after_terrain_change(&mut self) {
        if self.preserve_view {
            self.orbit_center = vf_scene::terrain_center(&self.heightfield);
            self.apply_orbit();
            self.sync_orbit_from_camera();
        } else {
            self.auto_frame();
        }
    }

    /// Snap the camera to a standard CAD viewpoint. The camera is placed
    /// immediately; the preview is refreshed by the adaptive scheduler.
    fn set_view_preset(&mut self, preset: vf_scene::ViewPreset) {
        self.orbit_center = vf_scene::terrain_center(&self.heightfield);
        let aspect = f64::from(self.render_width.max(1)) / f64::from(self.render_height.max(1));
        self.project.camera = vf_scene::framed_view(&self.heightfield, aspect, preset, self.fov);
        self.sync_orbit_from_camera();
        self.mark_camera_dirty();
        self.status = format!("Vue : {}", preset.label());
    }

    /// Return to the default three-quarter framing, preserving the current FOV.
    fn home_view(&mut self) {
        self.auto_frame();
        self.mark_camera_dirty();
        self.status = "Vue par défaut".into();
    }

    fn apply_gizmo_action(&mut self, action: view_gizmo::GizmoAction) {
        match action {
            view_gizmo::GizmoAction::View(preset) => self.set_view_preset(preset),
            view_gizmo::GizmoAction::Home => self.home_view(),
        }
    }

    /// CAD keyboard shortcuts: 1 face, 2 dessus, 3 droite, 0/Home défaut.
    fn view_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.text_edit_focused() {
            return;
        }
        let (undo, redo) = ctx.input(|input| {
            let command = input.modifiers.command;
            let shift = input.modifiers.shift;
            (
                command && !shift && input.key_pressed(egui::Key::Z),
                command
                    && (input.key_pressed(egui::Key::Y)
                        || (shift && input.key_pressed(egui::Key::Z))),
            )
        });
        if undo {
            self.undo_history(ctx);
            return;
        }
        if redo {
            self.redo_history(ctx);
            return;
        }
        let save = ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::S));
        if save {
            self.save_project(ctx);
            return;
        }
        let open = ctx.input(|input| input.modifiers.command && input.key_pressed(egui::Key::O));
        if open {
            self.open_project(ctx.clone());
            return;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::F1)) {
            self.help.toggle(help_topic_for_workspace(self.workspace));
            return;
        }
        let pressed = ctx.input(|input| {
            [
                input.key_pressed(egui::Key::Num1),
                input.key_pressed(egui::Key::Num2),
                input.key_pressed(egui::Key::Num3),
                input.key_pressed(egui::Key::Num0),
                input.key_pressed(egui::Key::Home),
            ]
        });
        if self.workspace == Workspace::Animation
            && ctx.input(|input| input.key_pressed(egui::Key::Space))
        {
            self.toggle_animation_play(ctx);
            return;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) && self.help.is_open() {
            self.help.close();
        }
        if self.workspace == Workspace::Animation
            && ctx.input(|input| input.key_pressed(egui::Key::Escape))
        {
            self.anim_editor.trace = false;
        }
        if self.workspace == Workspace::Animation
            && ctx.input(|input| {
                input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace)
            })
        {
            self.delete_animation_key();
        }
        if pressed[0] {
            self.set_view_preset(vf_scene::ViewPreset::Front);
        } else if pressed[1] {
            self.set_view_preset(vf_scene::ViewPreset::Top);
        } else if pressed[2] {
            self.set_view_preset(vf_scene::ViewPreset::Right);
        } else if pressed[3] || pressed[4] {
            self.home_view();
        }
    }

    fn invalidate_preview(&mut self, mesh_changed: bool) {
        let now = Instant::now();
        self.dirty_since = Some(now);
        // `dirty_since` is refreshed on every drag event; `dirty_first` keeps
        // the age of the oldest pending change so continuous orbiting still
        // produces fresh low-resolution frames instead of waiting for release.
        self.dirty_first.get_or_insert(now);
        self.refine_at = None;
        self.revision = self.revision.wrapping_add(1);
        self.preview_scale = if self.compare.enabled {
            // Comparison must stay at a readable resolution even while a
            // slider is dragged; the user explicitly chose the preview level.
            self.preview_quality.scale().clamp(MIN_PREVIEW_SCALE, 1.0)
        } else if self.adaptive_preview {
            self.interactive_scale
        } else {
            1.0
        };
        self.show_final_render = false;
        if mesh_changed {
            self.gpu_mesh_dirty = true;
            self.gpu_mesh_due = Some(now + Duration::from_millis(150));
        }
    }

    fn terrain_stats(&mut self) -> vf_core::TerrainStats {
        let stale = self.terrain_stats.is_none()
            || self
                .terrain_stats_hf
                .as_ref()
                .is_none_or(|cached| !Arc::ptr_eq(cached, &self.heightfield));
        if stale {
            self.terrain_stats = Some(self.heightfield.stats());
            self.terrain_stats_hf = Some(Arc::clone(&self.heightfield));
        }
        self.terrain_stats
            .unwrap_or_else(|| self.heightfield.stats())
    }

    fn invalidate_terrain_stats(&mut self) {
        self.terrain_stats_hf = None;
    }

    fn mark_dirty(&mut self) {
        self.hillshade_revision = self.hillshade_revision.wrapping_add(1);
        self.gpu_mesh_revision = self.gpu_mesh_revision.wrapping_add(1);
        self.invalidate_preview(true);
    }

    /// Camera-only change: no GPU mesh rebuild, no terrain upload.
    fn mark_camera_dirty(&mut self) {
        self.invalidate_preview(false);
    }

    fn gpu_active(&self) -> bool {
        self.gpu_enabled
            && self.gpu.is_some()
            && !self.show_final_render
            && !self.cpu_preview_forced
            && !self.compare.enabled
    }

    /// Enter before/after mode. The current project and terrain become the
    /// frozen "before" snapshot; the next CPU preview becomes the "after".
    fn compare_start(&mut self, ctx: &egui::Context) {
        if self.compare.enabled {
            return;
        }
        // Never reuse `self.texture` here: it may be a preview from another
        // workspace or an older project. The first completed CPU render after
        // this point is the authoritative "before" image.
        self.compare = CompareState {
            enabled: true,
            split: 0.5,
            before: None,
            baseline_project: Some(self.project.clone()),
            baseline_heightfield: Some(Arc::clone(&self.heightfield)),
            waiting_for_baseline: true,
            dragging: false,
        };
        // The GPU viewport cannot show a before/after split (it has no frozen
        // image), so compare mode always uses the CPU renderer. This is also
        // the only renderer that includes the continuous detail and the
        // material pass.
        self.texture = None;
        self.status =
            "Comparaison avant/après : le côté gauche est figé, le côté droit suit vos réglages."
                .into();
        if self.render_task.is_none() {
            if self.preview_quality == PreviewQuality::Final {
                self.start_render(ctx.clone(), true);
            } else {
                self.preview_scale = self.preview_quality.scale();
                self.start_render(ctx.clone(), false);
            }
        } else {
            // The in-flight render may be stale; queue a fresh baseline as
            // soon as the worker is free.
            self.dirty_since = Some(Instant::now());
            self.dirty_first = Some(Instant::now());
        }
    }

    /// Accept the pending edits and leave comparison mode.
    fn compare_apply(&mut self, ctx: &egui::Context) {
        if !self.compare.enabled {
            return;
        }
        self.compare = CompareState {
            split: 0.5,
            ..CompareState::default()
        };
        self.status = "Modifications appliquées".into();
        self.refresh_preview(ctx.clone());
    }

    /// Restore the snapshot taken when comparison started.
    fn compare_revert(&mut self, ctx: &egui::Context) {
        let Some(project) = self.compare.baseline_project.clone() else {
            self.compare = CompareState {
                split: 0.5,
                ..CompareState::default()
            };
            return;
        };
        let heightfield = self.compare.baseline_heightfield.clone();
        // Keep the current viewing pose: reverting a material/detail change
        // must not throw away a camera the user positioned to inspect it.
        let camera = self.project.camera.clone();
        self.compare = CompareState {
            split: 0.5,
            ..CompareState::default()
        };
        self.project = project;
        self.project.camera = camera;
        if let Some(heightfield) = heightfield {
            self.heightfield = heightfield;
        }
        self.texture = None;
        self.last_image = None;
        self.sculpt_texture = None;
        self.sculpt_texture_dirty = true;
        self.terrain_stats = None;
        self.terrain_stats_hf = None;
        self.anim_track = None;
        self.anim_track_error = None;
        self.anim_live_sample = None;
        self.anim_trail_cache = None;
        self.anim_compare_cache = None;
        self.anim_profile_cache = None;
        self.anim_curve_cache = None;
        self.anim_editor.selection = None;
        self.sync_from_project();
        self.sync_orbit_from_camera();
        self.history.clear(&self.project, &self.heightfield);
        self.status = "Modifications annulées : retour à l'état d'avant comparaison".into();
        self.refresh_preview(ctx.clone());
    }

    /// Start a fresh preview after loading/generating a scene. When the GPU
    /// viewport is active, only the mesh needs refreshing.
    fn refresh_preview(&mut self, ctx: egui::Context) {
        self.mark_dirty();
        if self.gpu_enabled && self.gpu.is_some() {
            self.gpu_mesh_due = Some(Instant::now());
            self.dirty_since = None;
            self.dirty_first = None;
            ctx.request_repaint();
        } else {
            self.preview_scale = if self.adaptive_preview {
                self.interactive_scale
            } else {
                1.0
            };
            self.start_render(ctx, false);
        }
    }

    /// A camera-only refresh (flight/rig scrubbing). The live GPU view reads
    /// the camera every frame; the CPU path still needs a preview render.
    fn refresh_camera_preview(&mut self, ctx: &egui::Context) {
        self.mark_camera_dirty();
        if self.gpu_enabled && self.gpu.is_some() {
            ctx.request_repaint();
        } else {
            self.start_render(ctx.clone(), false);
        }
    }

    /// Rebuild the GPU mesh after a debounce. Building on the UI thread is
    /// acceptable because it only happens after the user stops adjusting a
    /// parameter; camera orbit remains entirely on the GPU.
    fn pump_gpu(&mut self, ctx: &egui::Context) {
        if self.gpu.is_none() || !self.gpu_enabled {
            return;
        }
        if self.gpu_mesh_task.is_some() {
            ctx.request_repaint_after(Duration::from_millis(40));
            return;
        }
        if !self.gpu_mesh_dirty {
            return;
        }
        let Some(due) = self.gpu_mesh_due else {
            return;
        };
        if Instant::now() < due {
            ctx.request_repaint_after(Duration::from_millis(40));
            return;
        }
        let project = self.project.clone();
        let heightfield = Arc::clone(&self.heightfield);
        let water = self.water;
        let revision = self.gpu_mesh_revision;
        let (sender, receiver) = mpsc::channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = gpu_view::build_terrain_mesh(&project, &heightfield, water);
            let _ = sender.send(result);
            repaint.request_repaint();
        });
        self.gpu_mesh_task = Some(GpuMeshTask {
            receiver,
            revision,
            started: Instant::now(),
        });
        self.gpu_mesh_due = None;
        self.status = "Synchronisation du maillage GPU en arrière-plan…".into();
        ctx.request_repaint();
    }

    fn poll_gpu_mesh(&mut self, ctx: &egui::Context) {
        let Some(task) = &self.gpu_mesh_task else {
            return;
        };
        match task.receiver.try_recv() {
            Ok(Ok(data)) => {
                let revision = task.revision;
                let elapsed = task.started.elapsed();
                self.gpu_mesh_task = None;
                if !self.gpu_enabled || self.gpu.is_none() {
                    self.gpu_mesh_dirty = false;
                    self.gpu_mesh_due = None;
                    return;
                }
                if revision != self.gpu_mesh_revision {
                    // A newer edit superseded this mesh; rebuild on the next
                    // pump instead of uploading an outdated surface.
                    self.gpu_mesh_dirty = true;
                    self.gpu_mesh_due = Some(Instant::now());
                    ctx.request_repaint();
                    return;
                }
                match self.gpu.as_ref().map(|gpu| gpu.upload_mesh(data)) {
                    Some(Ok(())) => {
                        self.gpu_mesh_dirty = false;
                        self.gpu_mesh_due = None;
                        self.status =
                            format!("Maillage GPU synchronisé en {} ms", elapsed.as_millis());
                    }
                    Some(Err(error)) => {
                        self.gpu_error = Some(error.clone());
                        self.gpu_enabled = false;
                        self.gpu_mesh_dirty = false;
                        self.gpu_mesh_due = None;
                        self.mark_dirty();
                        self.gpu_mesh_dirty = false;
                        self.gpu_mesh_due = None;
                        self.status = format!("GPU désactivé : {error}");
                    }
                    None => {}
                }
            }
            Ok(Err(error)) => {
                self.gpu_mesh_task = None;
                self.gpu_error = Some(error.clone());
                self.gpu_enabled = false;
                self.gpu_mesh_dirty = false;
                self.gpu_mesh_due = None;
                self.status = format!("GPU désactivé : {error}");
                self.mark_dirty();
                self.gpu_mesh_dirty = false;
                self.gpu_mesh_due = None;
            }
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(40));
            }
            Err(TryRecvError::Disconnected) => {
                self.gpu_mesh_task = None;
                self.gpu_mesh_dirty = false;
                self.gpu_mesh_due = None;
                self.status = "Maillage GPU : thread interrompu".into();
            }
        }
    }

    fn new_project(&mut self, ctx: egui::Context) {
        if self.task.is_some() {
            self.status =
                "Attendez la fin de l'opération en cours avant de créer un projet.".into();
            return;
        }
        let (project, heightfield) = default_scene();
        self.project = project;
        self.heightfield = Arc::new(heightfield);
        self.history.clear(&self.project, &self.heightfield);
        self.project_path = None;
        self.path_input = "landscape.vfproj".into();
        self.status = "New project".into();
        self.texture = None;
        self.last_image = None;
        self.sculpt_texture = None;
        self.sculpt_texture_dirty = true;
        self.anim_track = None;
        self.anim_track_error = None;
        self.anim_live_sample = None;
        self.sync_from_project();
        self.sync_orbit_from_camera();
        self.refresh_preview(ctx);
    }

    fn open_project(&mut self, ctx: egui::Context) {
        let path = normalized_project_path(&self.path_input);
        if !path.exists() {
            self.status = format!("Open failed: {} does not exist", path.display());
            return;
        }
        let title = format!(
            "Chargement de {}…",
            path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned()
            )
        );
        if self.start_task(
            &ctx,
            jobs::TaskKind::LoadProject,
            title.clone(),
            move |progress| {
                progress.set_phase("Lecture de l'archive du projet…");
                progress.set_indeterminate();
                match vf_io::load_project(&path) {
                    Ok(loaded) => {
                        progress.set(1.0);
                        jobs::TaskResult::ProjectLoaded { loaded }
                    }
                    Err(error) => jobs::TaskResult::Failed(error.to_string()),
                }
            },
        ) {
            self.status = format!("{title} (l'interface reste utilisable)");
        }
    }

    /// Apply one undo step and refresh every cache that depends on the scene.
    fn undo_history(&mut self, ctx: &egui::Context) {
        if self.task.is_some() {
            self.status = "Attendez la fin de l'opération en cours avant d'annuler.".into();
            return;
        }
        if self.sculpt_stroke.is_some() {
            self.status = "Terminez le trait de pinceau avant d'annuler.".into();
            return;
        }
        // Drop the cached stats Arc before a sculpt-region undo: otherwise it
        // keeps the heightfield shared and `Arc::make_mut` clones the whole
        // terrain just to patch a small rectangle.
        self.invalidate_terrain_stats();
        let camera = self.project.camera.clone();
        let Some(label) = self.history.undo(&mut self.project, &mut self.heightfield) else {
            self.status = "Rien à annuler".into();
            return;
        };
        // History does not record camera moves; keep the inspection view.
        self.project.camera = camera;
        self.after_history_change(ctx, format!("Annulé : {label}"));
    }

    /// Apply one redo step.
    fn redo_history(&mut self, ctx: &egui::Context) {
        if self.task.is_some() {
            self.status = "Attendez la fin de l'opération en cours avant de rétablir.".into();
            return;
        }
        if self.sculpt_stroke.is_some() {
            self.status = "Terminez le trait de pinceau avant de rétablir.".into();
            return;
        }
        self.invalidate_terrain_stats();
        let camera = self.project.camera.clone();
        let Some(label) = self.history.redo(&mut self.project, &mut self.heightfield) else {
            self.status = "Rien à rétablir".into();
            return;
        };
        self.project.camera = camera;
        self.after_history_change(ctx, format!("Rétabli : {label}"));
    }

    fn after_history_change(&mut self, ctx: &egui::Context, status: String) {
        self.anim_playing = false;
        self.anim_editor.trace = false;
        self.anim_editor.selection = None;
        self.anim_edit_target = false;
        self.anim_framed = false;
        self.rig_time = self
            .rig_time
            .clamp(0.0, self.project.rig.duration_s.max(0.1));
        self.sculpt_stroke = None;
        self.texture = None;
        self.last_image = None;
        self.sculpt_texture = None;
        self.sculpt_texture_dirty = true;
        self.invalidate_terrain_stats();
        self.anim_track = None;
        self.anim_track_error = None;
        self.anim_live_sample = None;
        self.anim_trail_cache = None;
        self.anim_compare_cache = None;
        self.anim_profile_cache = None;
        self.anim_curve_cache = None;
        self.anim_camera_dirty = true;
        self.sync_from_project();
        self.sync_orbit_from_camera();
        self.status = status.clone();
        self.rig_status = status;
        self.refresh_preview(ctx.clone());
    }

    fn save_project(&mut self, ctx: &egui::Context) {
        let path = normalized_project_path(&self.path_input);
        if path.as_os_str().is_empty() {
            self.status = "Save failed: empty path".into();
            return;
        }
        let project = self.project.clone();
        let heightfield = Arc::clone(&self.heightfield);
        let preview = self.last_image.clone();
        let result_path = path.clone();
        let title = format!(
            "Enregistrement de {}…",
            path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned()
            )
        );
        if self.start_task(
            ctx,
            jobs::TaskKind::SaveProject,
            title.clone(),
            move |progress| {
                progress.set_phase("Préparation du manifeste…");
                progress.set_indeterminate();
                progress.set_phase("Compression du heightfield et écriture de l'archive…");
                progress.set_indeterminate();
                match vf_io::save_project(&project, &heightfield, &path, preview.as_ref()) {
                    Ok(()) => {
                        progress.set(1.0);
                        jobs::TaskResult::ProjectSaved { path: result_path }
                    }
                    Err(error) => jobs::TaskResult::Failed(error.to_string()),
                }
            },
        ) {
            self.status = format!("{title} (l'interface reste utilisable)");
        }
    }

    fn save_render(&mut self, ctx: &egui::Context) {
        let Some(image) = self.last_image.clone() else {
            self.status = "Nothing to save: render first".into();
            return;
        };
        let path = PathBuf::from(self.render_path.trim());
        let result_path = path.clone();
        let title = format!(
            "Écriture de l'image {}…",
            path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned()
            )
        );
        if self.start_task(
            ctx,
            jobs::TaskKind::SaveRender,
            title.clone(),
            move |progress| {
                progress.set_phase("Encodage PNG…");
                progress.set_indeterminate();
                match image.save_png(&path) {
                    Ok(()) => {
                        progress.set(1.0);
                        jobs::TaskResult::RenderSaved { path: result_path }
                    }
                    Err(error) => jobs::TaskResult::Failed(format!("PNG save failed: {error}")),
                }
            },
        ) {
            self.status = format!("{title} (l'interface reste utilisable)");
        }
    }

    fn generate_terrain(&mut self, ctx: egui::Context) {
        let seed = Seed::new(self.seed);
        let mut project = self.project.clone();
        project.seed = seed;
        let size = self.size.clamp(2, 16_384);
        project.source = match self.mode {
            TerrainMode::Classic => TerrainSource::Classic {
                params: ClassicParams {
                    seed: self.seed,
                    size,
                    fractal_dimension: self.dimension,
                    divisor: self.divisor,
                    island: self.island,
                    roughness: 1.0,
                    vertical_scale: self.vertical_scale,
                },
            },
            TerrainMode::Modern => {
                let mut params = match vf_core::terrain_params::modern_preset(&self.preset) {
                    Some(params) => params,
                    None => {
                        self.status = format!("Unknown preset '{}'", self.preset);
                        return;
                    }
                };
                params = params.with_size_seed(size, self.seed);
                params.vertical_scale = self.vertical_scale;
                if let Some(palette) = Palette::preset(&self.preset) {
                    project.palette = palette;
                }
                TerrainSource::Modern { params }
            }
        };
        let erosion = if self.project.erosion_enabled {
            Some(self.project.erosion.clone())
        } else {
            None
        };
        let aspect = f64::from(self.render_width) / f64::from(self.render_height.max(1));
        let task_seed = self.seed;
        let title = format!("Génération du terrain {size}²…");
        let started = self.start_task(
            &ctx,
            jobs::TaskKind::Generate,
            title.clone(),
            move |progress| {
                progress.set_phase("Génération du relief fractal…");
                progress.set_indeterminate();
                let begin = Instant::now();
                match vf_geology::build(&project.source, erosion.as_ref(), &project.geology, seed) {
                    Ok(heightfield) => {
                        progress
                            .set_phase("Érosion et géologie terminées, préparation de la caméra…");
                        progress.set(0.92);
                        let mut project = project;
                        project.sync_terrain(&heightfield);
                        project.camera = vf_scene::auto_frame(&heightfield, aspect);
                        progress.set(1.0);
                        jobs::TaskResult::TerrainGenerated {
                            project,
                            heightfield,
                            size,
                            seed: task_seed,
                            elapsed_ms: begin.elapsed().as_millis(),
                        }
                    }
                    Err(error) => jobs::TaskResult::Failed(error.to_string()),
                }
            },
        );
        if started {
            self.status = format!("{title} (l'interface reste utilisable)");
        }
    }

    fn full_options(&self) -> RenderOptions {
        let mut settings = self.project.render.clone();
        settings.width = self.render_width.clamp(64, 8192);
        settings.height = self.render_height.clamp(64, 8192);
        settings.quality = self.quality;
        settings.shadows = self.shadows;
        settings.ao = self.ao;
        settings.clouds = self.clouds;
        settings.water = self.water;
        settings.exposure = self.exposure;
        if self.quality != Quality::Custom {
            settings.ssaa = self.quality.presets().4;
            settings.shadow_resolution = self.quality.presets().5;
        }
        settings.cliffs = self.project.render.cliffs;
        settings.subdivision = self.project.render.subdivision;
        let mut options = RenderOptions::from(&settings);
        options.detail_geometry = self.project.detail.enabled;
        options
    }

    /// Preview options at a fraction of the 960x540 base size. The scale is
    /// capped on both axes so very small windows stay readable.
    fn preview_options_at(&self, scale: f32) -> RenderOptions {
        let scale = scale.clamp(MIN_PREVIEW_SCALE, 1.0);
        let width = ((PREVIEW_SIZE.0 as f32) * scale).round().max(160.0) as u32;
        let height = ((PREVIEW_SIZE.1 as f32) * scale).round().max(90.0) as u32;
        let mut options = RenderOptions::preview(width, height);
        options.shadows = self.shadows;
        options.ao = false;
        options.cliffs = self.project.render.cliffs;
        options.clouds = self.clouds;
        options.water = self.water;
        options.exposure = self.exposure;
        options.subdivision = self.project.render.subdivision;
        options.detail_geometry = self.project.detail.enabled;
        options
    }

    fn start_preview(&mut self, ctx: egui::Context, scale: f32) {
        if self.render_task.is_some() {
            return;
        }
        self.preview_scale = scale.clamp(MIN_PREVIEW_SCALE, 1.0);
        self.start_render(ctx, false);
    }

    /// Run the current preview at the quality selected in a panel. `Finale`
    /// switches to the project output resolution and the full render options.
    fn start_quality_preview(&mut self, ctx: egui::Context) {
        if self.preview_quality == PreviewQuality::Final {
            self.start_render(ctx, true);
        } else {
            self.preview_scale = self.preview_quality.scale();
            self.interactive_scale = self.preview_scale;
            self.start_render(ctx, false);
        }
    }

    /// Compact quality selector placed next to every "preview" action.
    fn preview_quality_picker(&mut self, ui: &mut egui::Ui) {
        egui::ComboBox::from_id_salt("preview-quality")
            .selected_text(format!("Aperçu {}", self.preview_quality.label()))
            .show_ui(ui, |ui| {
                for quality in PreviewQuality::ALL {
                    ui.selectable_value(
                        &mut self.preview_quality,
                        quality,
                        format!("{} — {}", quality.label(), quality.tooltip()),
                    );
                }
            });
    }

    fn start_render(&mut self, ctx: egui::Context, full: bool) {
        if let Some(task) = &self.render_task {
            if full && !task.full {
                self.pending_full = true;
                self.status = "Rendu final en attente de l'aperçu…".into();
                ctx.request_repaint();
            }
            return;
        }
        self.revision = self.revision.wrapping_add(1);
        let revision = self.revision;
        let (options, scale) = if full {
            (self.full_options(), 1.0)
        } else {
            let scale = if self.adaptive_preview {
                self.preview_scale.clamp(MIN_PREVIEW_SCALE, 1.0)
            } else {
                1.0
            };
            (self.preview_options_at(scale), scale)
        };
        self.preview_scale = scale;
        self.dirty_since = None;
        self.dirty_first = None;
        self.refine_at = None;
        if full {
            self.pending_full = false;
        }
        let project = self.project.clone();
        let heightfield = Arc::clone(&self.heightfield);
        let width = options.width;
        let height = options.height;
        let (sender, receiver) = mpsc::channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let result = render_with_options(&project, &heightfield, &options)
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
            repaint.request_repaint();
        });
        self.render_task = Some(RenderTask {
            receiver,
            started: Instant::now(),
            full,
            revision,
            scale,
        });
        self.status = if full {
            format!("Rendering {width}x{height}…")
        } else {
            format!("Aperçu {width}×{height}…")
        };
    }

    fn update_interactive_scale(&mut self, scale: f32, elapsed_ms: u128) {
        if !self.adaptive_preview || elapsed_ms == 0 {
            return;
        }
        self.interactive_scale = adapted_scale(self.interactive_scale, scale, elapsed_ms);
    }

    /// Start a low-resolution pass as soon as the pointer settles briefly,
    /// then progressively refine to full preview resolution while idle.
    fn pump_preview(&mut self, ctx: &egui::Context) {
        // When a GPU viewport exists, automatic CPU previews are unnecessary:
        // the GPU scene is live and a manual Render/Preview still works.
        if self.gpu_enabled && self.gpu.is_some() && !self.compare.enabled {
            return;
        }
        if self.render_task.is_some() {
            ctx.request_repaint_after(Duration::from_millis(30));
            return;
        }
        if self.dirty_since.is_some() {
            let now = Instant::now();
            let last = self.dirty_since.map_or(Duration::ZERO, |t| now - t);
            let first = self.dirty_first.map_or(last, |t| now - t);
            let delay = if self.adaptive_preview {
                PREVIEW_FIRST_DELAY
            } else {
                PREVIEW_SETTLE_DELAY
            };
            if last >= delay || first >= delay {
                self.dirty_since = None;
                self.dirty_first = None;
                let scale = if self.adaptive_preview {
                    self.preview_scale
                } else {
                    1.0
                };
                self.start_preview(ctx.clone(), scale);
            } else {
                ctx.request_repaint_after(Duration::from_millis(16));
            }
            return;
        }
        if let Some(at) = self.refine_at {
            if Instant::now() >= at {
                self.refine_at = None;
                let next = next_preview_scale(self.preview_scale);
                if next > self.preview_scale + 0.005 {
                    self.start_preview(ctx.clone(), next);
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(40));
            }
        }
    }

    /// Queue a heavy operation on a worker thread. Only one generic task runs
    /// at a time so two operations cannot fight over the same output file.
    fn start_task<F>(
        &mut self,
        ctx: &egui::Context,
        kind: jobs::TaskKind,
        title: impl Into<String>,
        work: F,
    ) -> bool
    where
        F: FnOnce(Arc<jobs::Progress>) -> jobs::TaskResult + Send + 'static,
    {
        if self.task.is_some() {
            self.status = "Une opération est déjà en cours : attendez la fin ou annulez-la.".into();
            return false;
        }
        // A discrete operation starts a new history step: commit whatever the
        // user was adjusting before it.
        self.history.flush(&self.project, &self.heightfield);
        self.task = Some(jobs::Task::spawn(ctx, kind, title, work));
        ctx.request_repaint();
        true
    }

    fn poll_task(&mut self, ctx: &egui::Context) {
        let Some((kind, poll)) = self.task.as_ref().map(|task| (task.kind, task.poll(ctx))) else {
            return;
        };
        match poll {
            jobs::Poll::Pending => {}
            jobs::Poll::Finished(result) => {
                self.task = None;
                self.apply_task_result(ctx, kind, *result);
            }
            jobs::Poll::Disconnected => {
                self.task = None;
                self.status = "Opération interrompue (le thread de travail s'est arrêté).".into();
            }
        }
    }

    fn apply_task_result(
        &mut self,
        ctx: &egui::Context,
        kind: jobs::TaskKind,
        result: jobs::TaskResult,
    ) {
        match result {
            jobs::TaskResult::Failed(error) => {
                let message = format!("Échec : {error}");
                self.status = message.clone();
                if matches!(
                    kind,
                    jobs::TaskKind::ImportTerrain
                        | jobs::TaskKind::ExportTerrain
                        | jobs::TaskKind::Blender
                        | jobs::TaskKind::AnimationExport
                        | jobs::TaskKind::AnimationImport
                ) {
                    self.files_status = message;
                }
            }
            jobs::TaskResult::ProjectLoaded { loaded } => {
                let path = loaded.path.clone();
                self.project = loaded.project;
                self.heightfield = Arc::new(loaded.heightfield);
                self.project_path = Some(path.clone());
                self.path_input = path.display().to_string();
                self.status = format!("Chargé {}", path.display());
                self.texture = None;
                self.last_image = loaded.preview;
                self.sculpt_texture = None;
                self.sculpt_texture_dirty = true;
                self.anim_track = None;
                self.anim_track_error = None;
                self.anim_live_sample = None;
                self.anim_trail_cache = None;
                self.anim_compare_cache = None;
                self.anim_profile_cache = None;
                self.anim_curve_cache = None;
                self.history.clear(&self.project, &self.heightfield);
                self.sync_from_project();
                self.sync_orbit_from_camera();
                self.refresh_preview(ctx.clone());
            }
            jobs::TaskResult::ProjectSaved { path } => {
                self.project_path = Some(path.clone());
                self.path_input = path.display().to_string();
                self.status = format!("Enregistré {}", path.display());
            }
            jobs::TaskResult::RenderSaved { path } => {
                self.status = format!("Image écrite : {}", path.display());
            }
            jobs::TaskResult::TerrainGenerated {
                project,
                heightfield,
                size,
                seed,
                elapsed_ms,
            } => {
                self.history
                    .label(format!("Génération {size}² seed {seed}"));
                self.project = project;
                self.heightfield = Arc::new(heightfield);
                self.texture = None;
                self.last_image = None;
                self.sculpt_texture = None;
                self.sculpt_texture_dirty = true;
                self.status = format!(
                    "Généré {} {size}x{size} seed {seed} en {elapsed_ms} ms",
                    match self.mode {
                        TerrainMode::Classic => "Classic",
                        TerrainMode::Modern => "Modern",
                    }
                );
                self.sync_from_project();
                self.adopt_view_after_terrain_change();
                self.refresh_preview(ctx.clone());
            }
            jobs::TaskResult::TerrainImported { path, imported } => {
                let (sx, sy) = imported.heightfield.spacing();
                let stem = path
                    .file_stem()
                    .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
                self.history.label("Import du terrain");
                self.project.source = imported.source;
                self.project.meta.name = if stem.is_empty() {
                    "Terrain importé".into()
                } else {
                    stem.clone()
                };
                self.project.legacy = Some(LegacyMeta {
                    dem_title: stem,
                    dem_comment: imported.description.clone(),
                    ..LegacyMeta::default()
                });
                self.heightfield = Arc::new(imported.heightfield);
                self.project.sync_terrain(&self.heightfield);
                self.project_path = None;
                self.path_input = path.with_extension("vfproj").display().to_string();
                self.anim_framed = false;
                self.anim_editor.selection = None;
                self.sculpt_texture = None;
                self.sculpt_texture_dirty = true;
                self.anim_track = None;
                self.anim_live_sample = None;
                self.anim_trail_cache = None;
                self.anim_compare_cache = None;
                self.anim_profile_cache = None;
                self.anim_curve_cache = None;
                self.sync_from_project();
                self.adopt_view_after_terrain_change();
                self.mark_dirty();
                self.start_render(ctx.clone(), false);
                self.files_status = format!(
                    "Importé {}×{} · {:.2} m/cellule · {}",
                    self.heightfield.width(),
                    self.heightfield.height(),
                    (sx + sy) * 0.5,
                    imported.description
                );
                self.status = self.files_status.clone();
            }
            jobs::TaskResult::TerrainExported { report } => {
                self.files_status = format!(
                    "Exporté {}×{} en {} ({}){}",
                    report.width,
                    report.height,
                    report.path.display(),
                    report.format,
                    if report.warnings.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", report.warnings.join(" "))
                    }
                );
                self.status = self.files_status.clone();
            }
            jobs::TaskResult::TerrainComposed { heightfield } => {
                self.history.label("Composition du monde");
                self.heightfield = Arc::new(heightfield);
                self.project.sync_terrain(&self.heightfield);
                self.texture = None;
                self.last_image = None;
                self.sculpt_texture = None;
                self.sculpt_texture_dirty = true;
                self.status = format!(
                    "Monde composé : {}×{}",
                    self.heightfield.width(),
                    self.heightfield.height()
                );
                self.sync_from_project();
                self.adopt_view_after_terrain_change();
                self.mark_dirty();
                self.start_render(ctx.clone(), false);
            }
            jobs::TaskResult::ModifiersBaked { heightfield } => {
                self.history.label("Cuisson des modificateurs");
                self.heightfield = Arc::new(heightfield);
                self.project.modifiers.clear();
                self.selected_modifier = 0;
                self.project.sync_terrain(&self.heightfield);
                self.texture = None;
                self.last_image = None;
                self.status = "Modificateurs cuits dans le heightfield".into();
                self.mark_dirty();
                self.start_render(ctx.clone(), false);
            }
            jobs::TaskResult::TerrainDensified {
                heightfield,
                old_width,
                new_width,
            } => {
                self.history.label("Densification du terrain");
                self.heightfield = Arc::new(heightfield);
                let new_cell = self
                    .heightfield
                    .spacing()
                    .0
                    .max(self.heightfield.spacing().1);
                self.project.sync_terrain(&self.heightfield);
                self.size = new_width as u32;
                self.sculpt_texture = None;
                self.sculpt_texture_dirty = true;
                self.mark_dirty();
                self.refresh_preview(ctx.clone());
                self.sculpt_status = format!(
                    "Terrain densifié : {old_width}² → {new_width}², cellule {new_cell:.2} m."
                );
            }
            jobs::TaskResult::BlenderExported { report, note } => {
                self.files_status = format!(
                    "Blender : {} sommets, {} triangles, {} matériaux → {}{}",
                    report.vertices,
                    report.triangles,
                    report.materials,
                    report.glb.display(),
                    note.map_or_else(String::new, |note| format!(" · {note}"))
                );
                self.status = self.files_status.clone();
            }
            jobs::TaskResult::AnimationExported { report } => {
                self.files_status = format!(
                    "Animation exportée : {} images, {:.2}s, {} → {}",
                    report.frames,
                    report.duration_s,
                    report.format,
                    report.path.display()
                );
                self.anim_io_path = report.path.display().to_string();
                self.rig_status = self.files_status.clone();
                self.status = self.files_status.clone();
            }
            jobs::TaskResult::AnimationImported { imported } => {
                let keys = imported.rig.eye.keys.len();
                let duration = imported.rig.duration_s;
                let terrain_note = imported
                    .terrain
                    .as_ref()
                    .map_or_else(String::new, |terrain| {
                        format!(
                            " · terrain d'origine {}×{} à {:.2} m/cellule",
                            terrain.width, terrain.height, terrain.spacing_x
                        )
                    });
                self.history.label("Import de l'animation");
                self.project.rig = imported.rig;
                self.project.rig.enabled = true;
                self.rig_time = 0.0;
                self.anim_editor.selection = None;
                self.anim_editor.trace = false;
                self.anim_edit_target = self.project.rig.target.mode == TargetMode::Path;
                self.anim_track = None;
                self.anim_live_sample = None;
                self.anim_trail_cache = None;
                self.anim_compare_cache = None;
                self.anim_profile_cache = None;
                self.anim_curve_cache = None;
                self.anim_camera_dirty = true;
                self.mark_animation_changed();
                self.files_status = format!(
                    "Animation importée : {keys} clés, {duration:.2}s, {}{terrain_note}",
                    imported.format
                );
                self.rig_status = self.files_status.clone();
                self.status = self.files_status.clone();
            }
        }
    }

    fn poll_render(&mut self, ctx: &egui::Context) {
        let Some(task) = self.render_task.take() else {
            return;
        };
        match task.receiver.try_recv() {
            Ok(Ok(output)) => {
                let stale = task.revision != self.revision;
                let gpu_available = self.gpu_enabled && self.gpu.is_some();
                if stale && self.compare.enabled && self.compare.waiting_for_baseline {
                    // Never freeze a stale frame as the comparison baseline:
                    // it would show a different scene on the left.
                    self.status = "Comparaison : référence obsolète, nouveau rendu…".into();
                    self.dirty_since = Some(Instant::now());
                    self.dirty_first = Some(Instant::now());
                } else if task.full && stale {
                    self.status = "Rendu final obsolète (scène modifiée)".into();
                } else if stale && gpu_available {
                    // The GPU view already shows the latest scene; do not
                    // replace it with an outdated CPU preview.
                    self.status = "Aperçu CPU obsolète (vue GPU active)".into();
                } else {
                    // During a drag an older low-resolution frame is still a
                    // useful intermediate image; it is replaced as soon as the
                    // queued low-resolution pass completes.
                    self.apply_render(ctx, output, &task);
                }
                self.last_render_ms = task.started.elapsed().as_millis();
                if !task.full {
                    self.preview_ms = self.last_render_ms;
                    if !stale {
                        self.preview_scale = task.scale;
                    }
                    self.update_interactive_scale(task.scale, self.last_render_ms);
                    if !stale && self.dirty_since.is_none() && task.scale < 0.999 {
                        self.refine_at = Some(Instant::now() + PREVIEW_REFINE_DELAY);
                    }
                }
                if self.pending_full {
                    self.pending_full = false;
                    self.start_render(ctx.clone(), true);
                }
            }
            Ok(Err(error)) => {
                self.status = format!("Render failed: {error}");
            }
            Err(TryRecvError::Empty) => {
                self.render_task = Some(task);
                ctx.request_repaint_after(Duration::from_millis(30));
            }
            Err(TryRecvError::Disconnected) => {
                self.status = "Render worker disconnected".into();
            }
        }
    }

    fn apply_render(&mut self, ctx: &egui::Context, output: RenderOutput, task: &RenderTask) {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [output.image.width as usize, output.image.height as usize],
            &output.image.pixels,
        );
        self.texture = Some(ctx.load_texture("viewport", image, egui::TextureOptions::LINEAR));
        if self.compare.enabled
            && (self.compare.waiting_for_baseline || self.compare.before.is_none())
        {
            self.compare.before = self.texture.clone();
            self.compare.waiting_for_baseline = false;
        }
        if self.capture_compare_stage == 1
            && self.compare.enabled
            && !self.compare.waiting_for_baseline
        {
            self.capture_compare_stage = 2;
        }
        self.last_render_size = (output.image.width, output.image.height);
        if task.full {
            self.status = format!(
                "Rendu {}×{} (ssaa {}) en {} ms",
                output.stats.width, output.stats.height, output.stats.ssaa, output.stats.elapsed_ms
            );
        } else {
            self.status = format!(
                "Aperçu {}×{} ({} %) · {} ms",
                output.stats.width,
                output.stats.height,
                (task.scale * 100.0).round() as u32,
                output.stats.elapsed_ms
            );
        }
        self.last_image = Some(output.image);
        // A completed CPU render becomes the displayed image until the scene
        // or camera changes; the next change switches back to the live GPU view.
        self.show_final_render = true;
    }

    fn terrain_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let header = egui::CollapsingHeader::new("Terrain")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Mode");
                    for mode in [TerrainMode::Classic, TerrainMode::Modern] {
                        if ui
                            .selectable_label(self.mode == mode, mode.label())
                            .clicked()
                        {
                            self.mode = mode;
                            self.mark_dirty();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Size");
                    if ui
                        .add(
                            egui::DragValue::new(&mut self.size)
                                .speed(2.0)
                                .range(2..=16_384),
                        )
                        .changed()
                    {
                        self.mark_dirty();
                    }
                    if ui.button("258").clicked() {
                        self.size = 258;
                        self.mark_dirty();
                    }
                    if ui.button("514").clicked() {
                        self.size = 514;
                        self.mark_dirty();
                    }
                    if ui.button("1026").clicked() {
                        self.size = 1026;
                        self.mark_dirty();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Seed");
                    ui.add(egui::DragValue::new(&mut self.seed).speed(1.0));
                    if ui.button("Random").clicked() {
                        self.seed = Seed::from_time().0;
                    }
                });
                if self.mode == TerrainMode::Modern {
                    ui.horizontal(|ui| {
                        ui.label("Preset");
                        let mut selected = self.preset.clone();
                        egui::ComboBox::from_id_salt("terrain-preset")
                            .selected_text(&self.preset)
                            .show_ui(ui, |ui| {
                                for name in vf_core::terrain_params::MODERN_PRESETS {
                                    ui.selectable_value(&mut selected, (*name).to_string(), *name);
                                }
                            });
                        if selected != self.preset {
                            self.preset = selected;
                            self.mark_dirty();
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Relief (m)");
                        if ui
                            .add(
                                egui::DragValue::new(&mut self.vertical_scale)
                                    .speed(10.0)
                                    .range(50.0..=6000.0),
                            )
                            .changed()
                        {
                            self.mark_dirty();
                        }
                    });
                } else {
                    ui.add(
                        egui::Slider::new(&mut self.dimension, 0.0..=200.0)
                            .text("Fractal dimension"),
                    );
                    ui.horizontal(|ui| {
                        ui.label("Divisor");
                        for divisor in [1u8, 2, 4, 8] {
                            if ui
                                .selectable_label(self.divisor == divisor, divisor.to_string())
                                .clicked()
                            {
                                self.divisor = divisor;
                                self.mark_dirty();
                            }
                        }
                        ui.checkbox(&mut self.island, "Island");
                    });
                    ui.horizontal(|ui| {
                        ui.label("Relief (m)");
                        if ui
                            .add(
                                egui::DragValue::new(&mut self.vertical_scale)
                                    .speed(10.0)
                                    .range(50.0..=6000.0),
                            )
                            .changed()
                        {
                            self.mark_dirty();
                        }
                    });
                }
                ui.checkbox(
                    &mut self.project.erosion_enabled,
                    "Hydraulic + thermal erosion",
                );
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(
                        &mut self.preserve_view,
                        "Conserver la caméra après génération / import",
                    )
                    .on_hover_text(
                        "Garde l'angle, la distance et le zoom de la vue courante : le pivot se replace au centre du nouveau terrain. Décochez pour recadrer automatiquement.",
                    );
                    if ui.button("Recadrer").clicked() {
                        self.auto_frame();
                        self.mark_camera_dirty();
                    }
                });
                if ui
                    .add_sized(
                        [ui.available_width(), 28.0],
                        egui::Button::new("Generate terrain"),
                    )
                    .clicked()
                {
                    self.generate_terrain(ctx.clone());
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Terrain,
        );
        self.world_ui(ui, ctx);
    }

    fn world_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let header = egui::CollapsingHeader::new("Monde et tuilage")
            .default_open(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Mode");
                    let mut mode = self.project.world.mode;
                    egui::ComboBox::from_id_salt("world-mode")
                        .selected_text(mode.name())
                        .show_ui(ui, |ui| {
                            for candidate in [
                                vf_core::WorldMode::Single,
                                vf_core::WorldMode::Tiled,
                                vf_core::WorldMode::Seamless,
                            ] {
                                ui.selectable_value(&mut mode, candidate, candidate.name());
                            }
                        });
                    self.project.world.mode = mode;
                });
                ui.horizontal(|ui| {
                    ui.label("Tuiles");
                    ui.add(
                        egui::DragValue::new(&mut self.project.world.tiles_x)
                            .speed(1.0)
                            .range(1..=16),
                    );
                    ui.label("x");
                    ui.add(
                        egui::DragValue::new(&mut self.project.world.tiles_y)
                            .speed(1.0)
                            .range(1..=16),
                    );
                });
                let (ow, oh) = self.project.world.output_size(
                    self.heightfield.width(),
                    self.heightfield.height(),
                );
                ui.label(
                    egui::RichText::new(format!(
                        "Sortie composée : {ow}×{oh} échantillons ({:.0} Mo)",
                        (ow * oh * 4) as f64 / 1_048_576.0
                    ))
                    .small()
                    .weak(),
                );
                ui.add(
                    egui::Slider::new(&mut self.project.world.blend_cells, 2..=512)
                        .text("Bande de raccord (échantillons)"),
                );
                ui.add(
                    egui::Slider::new(&mut self.project.world.macro_variation_m, 0.0..=300.0)
                        .text("Variation macro (m)"),
                );
                ui.checkbox(
                    &mut self.project.world.variation.flip_and_rotate,
                    "Flips / rotations des instances",
                );
                if ui
                    .add_sized(
                        [ui.available_width(), 26.0],
                        egui::Button::new("Composer le monde et remplacer le terrain"),
                    )
                    .clicked()
                {
                    let heightfield = Arc::clone(&self.heightfield);
                    let world = self.project.world.clone();
                    let seed = self.project.seed;
                    let title = "Composition du monde…".to_string();
                    if self.start_task(
                        ctx,
                        jobs::TaskKind::ComposeWorld,
                        title.clone(),
                        move |progress| {
                            progress.set_phase("Assemblage des tuiles et raccords…");
                            progress.set_indeterminate();
                            match vf_terrain::world::compose(&heightfield, &world, seed) {
                                Ok(heightfield) => {
                                    progress.set(1.0);
                                    jobs::TaskResult::TerrainComposed { heightfield }
                                }
                                Err(error) => {
                                    jobs::TaskResult::Failed(format!("Monde : {error}"))
                                }
                            }
                        },
                    ) {
                        self.status = format!("{title} (l'interface reste utilisable)");
                    }
                }
                ui.label(
                    egui::RichText::new(
                        "Le terrain stocké devient le monde composé. Rechargez le projet                          ou régénérez avant de recomposer.",
                    )
                    .small()
                    .weak(),
                );
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::World,
        );
    }

    fn bake_modifiers(&mut self, ctx: &egui::Context) {
        if self.project.modifiers.is_empty() {
            self.status = "Modifier stack is empty".into();
            return;
        }
        let stack = self.project.modifiers.clone();
        let heightfield = Arc::clone(&self.heightfield);
        let seed = self.project.seed;
        let title = "Cuisson des modificateurs…".to_string();
        if self.start_task(
            ctx,
            jobs::TaskKind::BakeModifiers,
            title.clone(),
            move |progress| {
                progress.set_phase("Application de la pile au heightfield…");
                progress.set_indeterminate();
                match vf_modifiers::apply(&heightfield, &stack, seed) {
                    Ok(heightfield) => {
                        progress.set(1.0);
                        jobs::TaskResult::ModifiersBaked { heightfield }
                    }
                    Err(error) => jobs::TaskResult::Failed(format!("Bake failed: {error}")),
                }
            },
        ) {
            self.status = format!("{title} (l'interface reste utilisable)");
        }
    }

    fn modifiers_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        #[derive(Clone, Copy)]
        enum Action {
            Up(usize),
            Down(usize),
            Remove(usize),
        }
        let mut changed = false;
        let mut action = None;

        let header = egui::CollapsingHeader::new("Pile de modificateurs")
            .default_open(true)
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(
                        "Les modificateurs s'appliquent au rendu, dans l'ordre de la pile. \
                         « Cuire » les intègre définitivement au terrain enregistré.",
                    )
                    .small()
                    .weak(),
                );
                ui.horizontal(|ui| {
                    let mut selected = self.add_modifier_kind;
                    egui::ComboBox::from_id_salt("add-modifier")
                        .selected_text(selected.label())
                        .width(190.0)
                        .show_ui(ui, |ui| {
                            for group in [
                                "Déformation",
                                "Sculpture",
                                "Détail de roche",
                                "Érosion",
                                "Combinaison",
                            ] {
                                ui.label(egui::RichText::new(group).small().weak());
                                for kind in ModifierKind::ALL
                                    .into_iter()
                                    .filter(|kind| kind.group() == group)
                                {
                                    ui.selectable_value(&mut selected, kind, kind.label());
                                }
                            }
                        });
                    self.add_modifier_kind = selected;
                    if ui.button("+ Ajouter").clicked() {
                        let index = self.project.modifiers.push(selected);
                        self.selected_modifier = index;
                        self.status = format!("Added {}", selected.label());
                        changed = true;
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Cuire dans le terrain").clicked() {
                        self.bake_modifiers(ctx);
                    }
                    if ui.button("Vider").clicked() {
                        self.project.modifiers.clear();
                        self.selected_modifier = 0;
                        changed = true;
                    }
                    if ui.button("Aperçu").clicked() {
                        self.start_quality_preview(ctx.clone());
                    }
                });
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Modifiers,
        );

        ui.add_space(6.0);
        let count = self.project.modifiers.items.len();
        if count == 0 {
            ui.label(egui::RichText::new("Pile vide.").weak());
        }
        for index in 0..count {
            let selected = self.selected_modifier == index;
            ui.horizontal(|ui| {
                let item = &mut self.project.modifiers.items[index];
                if ui.checkbox(&mut item.enabled, "").changed() {
                    changed = true;
                }
                let label = format!("{index}. {}", item.kind.label());
                if ui.selectable_label(selected, label).clicked() {
                    self.selected_modifier = index;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").clicked() {
                        action = Some(Action::Remove(index));
                    }
                    if ui.small_button("▼").clicked() {
                        action = Some(Action::Down(index));
                    }
                    if ui.small_button("▲").clicked() {
                        action = Some(Action::Up(index));
                    }
                });
            });
        }

        if self.selected_modifier < count {
            let index = self.selected_modifier;
            ui.separator();
            let mut item_changed = false;
            if let Some(item) = self.project.modifiers.items.get_mut(index) {
                ui.label(egui::RichText::new(item.summary()).strong());
                ui.add_space(4.0);
                let specs = item.specs();
                for spec in specs {
                    let mut row_changed = false;
                    let mut value = item.value(spec.key);
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [150.0, 18.0],
                            egui::Label::new(egui::RichText::new(spec.label).size(12.5)).truncate(),
                        );
                        if !spec.choices.is_empty() {
                            let current = spec
                                .choices
                                .iter()
                                .find(|(choice, _)| (*choice - value).abs() < 0.5)
                                .map_or("?", |(_, label)| *label);
                            egui::ComboBox::from_id_salt(format!(
                                "mod-choice-{index}-{}",
                                spec.key
                            ))
                            .selected_text(current)
                            .width(170.0)
                            .show_ui(ui, |ui| {
                                for (choice, label) in spec.choices {
                                    if ui
                                        .selectable_label((value - *choice).abs() < 0.5, *label)
                                        .clicked()
                                    {
                                        value = *choice;
                                        row_changed = true;
                                    }
                                }
                            });
                        } else {
                            let width = (ui.available_width() - 92.0).max(60.0);
                            let mut slider = egui::Slider::new(&mut value, spec.min..=spec.max)
                                .show_value(false);
                            if spec.logarithmic {
                                slider = slider.logarithmic(true);
                            }
                            if spec.integer {
                                slider = slider.step_by(1.0);
                            }
                            if ui.add_sized([width, 18.0], slider).changed() {
                                row_changed = true;
                            }
                            let mut drag = egui::DragValue::new(&mut value)
                                .speed(((spec.max - spec.min) / 200.0).max(0.0001))
                                .range(spec.min..=spec.max);
                            if !spec.unit.is_empty() {
                                drag = drag.suffix(format!(" {}", spec.unit));
                            }
                            if ui.add_sized([78.0, 18.0], drag).changed() {
                                row_changed = true;
                            }
                        }
                    });
                    if row_changed {
                        item.set(spec.key, value);
                        item_changed = true;
                    }
                }
            }
            changed |= item_changed;
        }

        match action {
            Some(Action::Up(index)) => {
                if self.project.modifiers.move_up(index) {
                    self.selected_modifier = index.saturating_sub(1);
                    changed = true;
                }
            }
            Some(Action::Down(index)) => {
                if self.project.modifiers.move_down(index) {
                    self.selected_modifier = index + 1;
                    changed = true;
                }
            }
            Some(Action::Remove(index)) => {
                self.project.modifiers.remove(index);
                self.selected_modifier = self
                    .selected_modifier
                    .min(self.project.modifiers.items.len().saturating_sub(1));
                changed = true;
            }
            None => {}
        }

        if changed {
            self.project.mark_modified();
            self.mark_dirty();
        }
    }

    fn detail_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("Voir le détail en avant/après (CPU)")
                .on_hover_text(
                    "Le viewport GPU est un aperçu de navigation : il n'applique pas le détail continu. Le rendu CPU le montre ; ce bouton le compare à l'état figé.",
                )
                .clicked()
            {
                self.cpu_preview_forced = true;
                self.compare_start(ctx);
            }
            ui.label(
                egui::RichText::new("Le détail et le maillage adaptatif sont visibles en vue CPU.")
                    .small()
                    .weak(),
            );
        });
        let header = egui::CollapsingHeader::new("Géométrie")
            .default_open(true)
            .show(ui, |ui| {
                ui.checkbox(&mut self.project.detail.enabled, "Détail continu actif");
                ui.horizontal(|ui| {
                    ui.label("Profil de roche");
                    let mut selected = self.project.detail.profile.clone();
                    egui::ComboBox::from_id_salt("detail-profile")
                        .selected_text(&selected)
                        .show_ui(ui, |ui| {
                            for name in vf_core::DETAIL_PRESETS {
                                ui.selectable_value(&mut selected, (*name).to_string(), *name);
                            }
                        });
                    if selected != self.project.detail.profile
                        && let Some(preset) = vf_core::DetailSettings::preset(&selected)
                    {
                        let seed = self.project.detail.seed;
                        self.project.detail = preset;
                        self.project.detail.seed = seed;
                        self.project.detail.enabled = true;
                        self.mark_dirty();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Densité du maillage");
                    let mut subdivision = self.project.render.subdivision as f32;
                    if ui
                        .add(
                            egui::Slider::new(&mut subdivision, 1.0..=4.0)
                                .step_by(1.0)
                                .text("×"),
                        )
                        .changed()
                    {
                        self.project.render.subdivision = subdivision.round() as u8;
                        self.mark_dirty();
                    }
                });
                ui.separator();
                ui.label(egui::RichText::new("Maillage adaptatif").strong());
                let mut adaptive = self.project.detail.adaptive;
                if ui
                    .checkbox(
                        &mut adaptive,
                        "Adapter localement le maillage à la forme",
                    )
                    .on_hover_text(
                        "Comme les couches adaptatives d'un slicer : les zones plates deviennent de grandes faces, les crêtes et falaises reçoivent les sommets. L'erreur est bornée par la tolérance.",
                    )
                    .changed()
                {
                    self.project.detail.adaptive = adaptive;
                    self.mark_dirty();
                }
                if self.project.detail.adaptive {
                    if param_f32(
                        ui,
                        "Tolérance géométrique",
                        "m",
                        &mut self.project.detail.adaptive_tolerance_m,
                        0.05,
                        50.0,
                        2.5,
                    ) {
                        self.mark_dirty();
                    }
                    ui.label(
                        egui::RichText::new(
                            "Petite tolérance = plus de sommets, plus fidèle. Les UV restent la projection monde, continues sur tout le terrain.",
                        )
                        .small()
                        .weak(),
                    );
                }
                if ui
                    .checkbox(
                        &mut self.project.detail.quad_mesh,
                        "Exporter en quads / n-gons (OBJ Blender)",
                    )
                    .on_hover_text(
                        "Le GLB reste triangulé (glTF ne connaît pas les quads) ; le fichier scene.obj du dossier Blender conserve les quads et les raccords.",
                    )
                    .changed()
                {
                    self.mark_dirty();
                }
                if param_f32(
                    ui,
                    "Intensité",
                    "",
                    &mut self.project.detail.strength,
                    0.0,
                    2.0,
                    1.0,
                ) {
                    self.mark_dirty();
                }
                if param_f32(
                    ui,
                    "Échelle fractale",
                    "m",
                    &mut self.project.detail.scale_m,
                    0.5,
                    500.0,
                    45.0,
                ) {
                    self.mark_dirty();
                }
                if param_f32(
                    ui,
                    "Amplitude",
                    "m",
                    &mut self.project.detail.amplitude_m,
                    0.0,
                    30.0,
                    1.2,
                ) {
                    self.mark_dirty();
                }
                if param_f32(
                    ui,
                    "Dimension fractale",
                    "",
                    &mut self.project.detail.fractal_dimension,
                    2.0,
                    3.0,
                    2.25,
                ) {
                    self.mark_dirty();
                }
                let mut octaves = self.project.detail.octaves as f32;
                if param_f32(ui, "Octaves", "", &mut octaves, 1.0, 12.0, 7.0) {
                    self.project.detail.octaves = octaves.round().clamp(1.0, 12.0) as u32;
                    self.mark_dirty();
                }
                if param_f32(
                    ui,
                    "Hétérogénéité",
                    "",
                    &mut self.project.detail.heterogeneity,
                    0.0,
                    1.0,
                    0.6,
                ) {
                    self.mark_dirty();
                }
                if param_f32(
                    ui,
                    "Gain",
                    "",
                    &mut self.project.detail.gain,
                    0.0,
                    0.95,
                    0.5,
                ) {
                    self.mark_dirty();
                }
                if param_f32(
                    ui,
                    "Warp",
                    "",
                    &mut self.project.detail.warp,
                    0.0,
                    1.0,
                    0.35,
                ) {
                    self.mark_dirty();
                }
                ui.checkbox(
                    &mut self.project.detail.band_limit,
                    "Anti-aliasing par empreinte pixel",
                );
                if ui
                    .add(egui::Button::new("Rendre l'aperçu à jour"))
                    .clicked()
                {
                    self.start_quality_preview(ctx.clone());
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Detail,
        );

        let header = egui::CollapsingHeader::new("Sillons et arêtes")
            .default_open(false)
            .show(ui, |ui| {
                ui.checkbox(&mut self.project.detail.grooves.enabled, "Sillons");
                if param_f32(
                    ui,
                    "Espacement",
                    "m",
                    &mut self.project.detail.grooves.spacing_m,
                    0.5,
                    500.0,
                    18.0,
                ) {
                    self.mark_dirty();
                }
                if param_f32(
                    ui,
                    "Profondeur",
                    "m",
                    &mut self.project.detail.grooves.depth_m,
                    0.0,
                    20.0,
                    0.8,
                ) {
                    self.mark_dirty();
                }
                if param_f32(
                    ui,
                    "Orientation",
                    "°",
                    &mut self.project.detail.grooves.angle_deg,
                    -180.0,
                    180.0,
                    90.0,
                ) {
                    self.mark_dirty();
                }
                ui.checkbox(&mut self.project.detail.ridges.enabled, "Arêtes");
                if param_f32(
                    ui,
                    "Hauteur des arêtes",
                    "m",
                    &mut self.project.detail.ridges.amplitude_m,
                    0.0,
                    20.0,
                    0.4,
                ) {
                    self.mark_dirty();
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Detail,
        );

        let header = egui::CollapsingHeader::new("Cratères, plaques, vésicules")
            .default_open(false)
            .show(ui, |ui| {
                ui.checkbox(&mut self.project.detail.craters.enabled, "Cratères");
                if param_f32(
                    ui,
                    "Densité",
                    "",
                    &mut self.project.detail.craters.density,
                    0.0,
                    1.0,
                    0.35,
                ) {
                    self.mark_dirty();
                }
                if param_f32(
                    ui,
                    "Rayon",
                    "m",
                    &mut self.project.detail.craters.radius_m,
                    0.2,
                    500.0,
                    14.0,
                ) {
                    self.mark_dirty();
                }
                ui.checkbox(&mut self.project.detail.exfoliation.enabled, "Exfoliation");
                ui.checkbox(&mut self.project.detail.vesicles.enabled, "Vésicules");
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Detail,
        );

        let header = egui::CollapsingHeader::new("Gros plan")
            .default_open(true)
            .show(ui, |ui| {
                ui.checkbox(&mut self.project.detail.focus_enabled, "Patch haute densité");
                ui.horizontal(|ui| {
                    ui.label("Centre");
                    ui.add(
                        egui::DragValue::new(&mut self.project.detail.focus_center[0])
                            .speed(1.0)
                            .prefix("x "),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.project.detail.focus_center[1])
                            .speed(1.0)
                            .prefix("y "),
                    );
                });
                if param_f32(
                    ui,
                    "Taille du patch",
                    "m",
                    &mut self.project.detail.focus_size_m,
                    8.0,
                    2000.0,
                    240.0,
                ) {
                    self.mark_dirty();
                }
                if param_f32(
                    ui,
                    "Pas du patch",
                    "m",
                    &mut self.project.detail.focus_spacing_m,
                    0.02,
                    10.0,
                    0.35,
                ) {
                    self.mark_dirty();
                }
                ui.label(
                    egui::RichText::new(
                        "Place la caméra près du centre pour voir le détail. Le patch                          est retiré du maillage grossier pour éviter le z-fighting.",
                    )
                    .small()
                    .weak(),
                );
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Detail,
        );
    }

    fn rock_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let header = egui::CollapsingHeader::new("Famille de roche")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Preset");
                    let mut selected = self.rock_preset.clone();
                    egui::ComboBox::from_id_salt("rock-preset")
                        .selected_text(&self.rock_preset)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut selected, "none".to_string(), "none");
                            for name in vf_core::GEOLOGY_PRESETS {
                                ui.selectable_value(&mut selected, (*name).to_string(), *name);
                            }
                        });
                    if selected != self.rock_preset {
                        self.rock_preset = selected.clone();
                        if let Some(settings) = vf_core::GeologySettings::preset(&selected) {
                            if settings.auto_palette
                                && let Some(palette) = Palette::preset(settings.suggested_palette())
                            {
                                self.project.palette = palette;
                            }
                            self.project.geology = settings;
                            if let Some(detail) = vf_core::DetailSettings::preset(&selected) {
                                let seed = self.project.detail.seed;
                                self.project.detail = detail;
                                self.project.detail.seed = seed;
                            }
                            self.mark_dirty();
                        }
                    }
                });
                ui.checkbox(
                    &mut self.project.geology.enabled,
                    "Appliquer à la génération",
                );
                ui.checkbox(
                    &mut self.project.geology.auto_palette,
                    "Palette assortie au preset",
                );
                ui.label(
                    egui::RichText::new(
                        "La géologie est appliquée après l'érosion, puis stabilisée.",
                    )
                    .small()
                    .weak(),
                );
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Rock,
        );
        let header = egui::CollapsingHeader::new("Bancs")
            .default_open(true)
            .show(ui, |ui| {
                let g = &mut self.project.geology;
                let mut changed = false;
                changed |= param_f32(
                    ui,
                    "Épaisseur",
                    "m",
                    &mut g.strata.thickness_m,
                    1.0,
                    120.0,
                    30.0,
                );
                changed |= param_f32(ui, "Pendage", "°", &mut g.strata.dip_deg, 0.0, 80.0, 4.0);
                changed |= param_f32(
                    ui,
                    "Azimut pendage",
                    "°",
                    &mut g.strata.dip_azimuth_deg,
                    0.0,
                    360.0,
                    90.0,
                );
                changed |= param_f32(
                    ui,
                    "Dureté",
                    "",
                    &mut g.strata.hardness_contrast,
                    0.0,
                    1.0,
                    0.6,
                );
                changed |= param_f32(ui, "Relief", "m", &mut g.strata.relief_m, 0.0, 40.0, 8.0);
                changed |= param_f32(ui, "Déformation", "", &mut g.strata.warp, 0.0, 2.0, 0.5);
                changed |= param_f32(
                    ui,
                    "Échelle déform.",
                    "m",
                    &mut g.strata.warp_scale_m,
                    50.0,
                    5000.0,
                    700.0,
                );
                changed |= param_f32(
                    ui,
                    "Couleur",
                    "",
                    &mut g.strata.color_contrast,
                    0.0,
                    1.0,
                    0.45,
                );
                if changed {
                    self.mark_dirty();
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Rock,
        );
        let header = egui::CollapsingHeader::new("Fractures")
            .default_open(true)
            .show(ui, |ui| {
                let g = &mut self.project.geology;
                let mut changed = false;
                ui.checkbox(&mut g.joints.enabled, "Activer");
                ui.horizontal(|ui| {
                    ui.label("Réseau");
                    let mut pattern = g.joints.pattern;
                    egui::ComboBox::from_id_salt("joint-pattern")
                        .selected_text(match pattern {
                            vf_core::JointPattern::Orthogonal => "orthogonal",
                            vf_core::JointPattern::HexColumns => "colonnes",
                            vf_core::JointPattern::Radial => "radial",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut pattern,
                                vf_core::JointPattern::Orthogonal,
                                "orthogonal",
                            );
                            ui.selectable_value(
                                &mut pattern,
                                vf_core::JointPattern::HexColumns,
                                "colonnes",
                            );
                            ui.selectable_value(
                                &mut pattern,
                                vf_core::JointPattern::Radial,
                                "radial",
                            );
                        });
                    if pattern != g.joints.pattern {
                        g.joints.pattern = pattern;
                        changed = true;
                    }
                });
                changed |= param_f32(ui, "Maille", "m", &mut g.joints.spacing_m, 2.0, 200.0, 40.0);
                changed |= param_f32(ui, "Profondeur", "m", &mut g.joints.depth_m, 0.0, 20.0, 3.0);
                changed |= param_f32(ui, "Largeur", "", &mut g.joints.width, 0.02, 0.5, 0.12);
                if changed {
                    self.mark_dirty();
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Rock,
        );
        let header = egui::CollapsingHeader::new("Dissolution")
            .default_open(false)
            .show(ui, |ui| {
                let g = &mut self.project.geology;
                let mut changed = false;
                ui.checkbox(&mut g.dissolution.enabled, "Activer");
                changed |= param_f32(ui, "Densité", "", &mut g.dissolution.density, 0.0, 1.0, 0.4);
                changed |= param_f32(
                    ui,
                    "Rayon",
                    "m",
                    &mut g.dissolution.radius_m,
                    5.0,
                    400.0,
                    90.0,
                );
                changed |= param_f32(
                    ui,
                    "Profondeur",
                    "m",
                    &mut g.dissolution.depth_m,
                    0.0,
                    40.0,
                    12.0,
                );
                changed |= param_f32(
                    ui,
                    "Cannelures",
                    "",
                    &mut g.dissolution.fluting,
                    0.0,
                    1.0,
                    0.4,
                );
                changed |= param_f32(ui, "Détail surface", "m", &mut g.detail_m, 0.0, 8.0, 1.0);
                if changed {
                    self.mark_dirty();
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Rock,
        );
        let header = egui::CollapsingHeader::new("Canyon")
            .default_open(false)
            .show(ui, |ui| {
                let g = &mut self.project.geology;
                let mut changed = false;
                ui.checkbox(&mut g.canyon.enabled, "Creuser un canyon");
                changed |= param_f32(
                    ui,
                    "Profondeur",
                    "m",
                    &mut g.canyon.depth_m,
                    0.0,
                    800.0,
                    260.0,
                );
                changed |= param_f32(
                    ui,
                    "Demi-largeur",
                    "m",
                    &mut g.canyon.width_m,
                    50.0,
                    3000.0,
                    1100.0,
                );
                changed |= param_f32(ui, "Méandres", "", &mut g.canyon.sinuosity, 0.0, 1.0, 0.5);
                changed |= param_f32(
                    ui,
                    "Cannelures",
                    "m",
                    &mut g.gullies.depth_m,
                    0.0,
                    40.0,
                    9.0,
                );
                if changed {
                    self.mark_dirty();
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Rock,
        );
        if ui
            .add_sized(
                [ui.available_width(), 28.0],
                egui::Button::new("Appliquer la géologie (régénérer)"),
            )
            .clicked()
        {
            self.generate_terrain(ctx.clone());
        }
    }

    fn light_ui(&mut self, ui: &mut egui::Ui) {
        let header = egui::CollapsingHeader::new("Soleil et ciel")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Palette");
                    let mut selected = self.project.palette.name.clone();
                    egui::ComboBox::from_id_salt("palette")
                        .selected_text(&self.project.palette.name)
                        .show_ui(ui, |ui| {
                            for name in [
                                "alpine", "desert", "volcanic", "coastal", "canyon", "lunar",
                                "island",
                            ] {
                                ui.selectable_value(&mut selected, name.to_string(), name);
                            }
                        });
                    if selected != self.project.palette.name
                        && let Some(palette) = Palette::preset(&selected)
                    {
                        self.project.palette = palette;
                        self.mark_dirty();
                    }
                });
                let mut changed = false;
                changed |= param_f32(
                    ui,
                    "Azimut",
                    "°",
                    &mut self.project.sun.azimuth_deg_from_south,
                    0.0,
                    360.0,
                    315.0,
                );
                changed |= param_f32(
                    ui,
                    "Déclinaison",
                    "°",
                    &mut self.project.sun.declination_deg,
                    0.0,
                    90.0,
                    35.0,
                );
                changed |= param_f32(
                    ui,
                    "Intensité",
                    "",
                    &mut self.project.sun.intensity,
                    0.0,
                    3.0,
                    1.15,
                );
                changed |= param_f32(
                    ui,
                    "Température",
                    "K",
                    &mut self.project.sun.color_k,
                    1000.0,
                    20000.0,
                    5600.0,
                );
                if changed {
                    self.mark_dirty();
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Light,
        );
        let header = egui::CollapsingHeader::new("Atmosphère")
            .default_open(true)
            .show(ui, |ui| {
                let mut changed = false;
                changed |= param_f32(
                    ui,
                    "Brume",
                    "",
                    &mut self.project.atmosphere.haze_density,
                    0.0,
                    0.0002,
                    0.000_012,
                );
                changed |= param_f32(
                    ui,
                    "Ambiante",
                    "",
                    &mut self.project.atmosphere.ambient_strength,
                    0.0,
                    1.0,
                    0.30,
                );
                changed |= param_f32(ui, "Exposition", "", &mut self.exposure, 0.2, 3.0, 1.0);
                if changed {
                    self.project.render.exposure = self.exposure;
                    self.mark_dirty();
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Light,
        );
    }

    fn water_ui(&mut self, ui: &mut egui::Ui) {
        let header = egui::CollapsingHeader::new("Mer et eau")
            .default_open(true)
            .show(ui, |ui| {
                ui.checkbox(&mut self.project.water.enabled, "Eau activée");
                let stats = self.terrain_stats();
                let (min, max) = (stats.min, stats.max);
                let mut changed = false;
                changed |= param_f32(
                    ui,
                    "Niveau de mer",
                    "m",
                    &mut self.project.water.sea_level,
                    min,
                    max,
                    0.0,
                );
                changed |= param_f32(
                    ui,
                    "Vagues",
                    "",
                    &mut self.project.water.wave_strength,
                    0.0,
                    2.0,
                    0.35,
                );
                changed |= param_f32(
                    ui,
                    "Échelle vagues",
                    "",
                    &mut self.project.water.wave_scale,
                    0.0,
                    1.0,
                    0.06,
                );
                changed |= param_f32(
                    ui,
                    "Opacité",
                    "",
                    &mut self.project.water.opacity,
                    0.0,
                    1.0,
                    0.88,
                );
                changed |= param_f32(ui, "Écume", "", &mut self.project.water.foam, 0.0, 1.0, 0.6);
                if changed {
                    self.mark_dirty();
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Water,
        );
    }

    fn materials_ui(&mut self, ui: &mut egui::Ui) {
        let header = egui::CollapsingHeader::new("Matières")
            .default_open(true)
            .show(ui, |ui| {
                ui.checkbox(&mut self.project.materials.enabled, "Matériaux activés");
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button("Voir en avant/après (CPU)")
                        .on_hover_text(
                            "Le viewport GPU n'affiche ni le détail continu ni les matières. Ce bouton bascule sur le rendu CPU avec une comparaison figée.",
                        )
                        .clicked()
                    {
                        self.cpu_preview_forced = true;
                        self.compare_start(ui.ctx());
                    }
                });
                let m = &mut self.project.materials;
                let mut changed = false;
                ui.horizontal(|ui| {
                    ui.label("Roche procédurale");
                    let mut preset = m.rock_preset.clone();
                    egui::ComboBox::from_id_salt("rock-preset")
                        .selected_text(&preset)
                        .show_ui(ui, |ui| {
                            for name in vf_core::ROCK_PRESETS {
                                ui.selectable_value(&mut preset, (*name).to_string(), *name);
                            }
                        });
                    if preset != m.rock_preset {
                        m.rock_preset = preset;
                        changed = true;
                    }
                });
                changed |= param_f32(ui, "Roche", "", &mut m.rock, 0.0, 1.0, 0.8);
                ui.separator();
                ui.label(egui::RichText::new("Bump et déplacement").strong());
                changed |= param_f32(
                    ui,
                    "Bump",
                    "",
                    &mut m.bump_strength,
                    0.0,
                    2.0,
                    0.6,
                );
                changed |= param_f32(
                    ui,
                    "Échelle bump",
                    "m",
                    &mut m.bump_scale_m,
                    0.05,
                    100.0,
                    7.0,
                );
                changed |= param_f32(
                    ui,
                    "Déplacement",
                    "m",
                    &mut m.displacement_m,
                    0.0,
                    30.0,
                    0.0,
                );
                ui.separator();
                ui.label(egui::RichText::new("Texture personnelle").strong());
                ui.horizontal(|ui| {
                    ui.label("Fichier");
                    changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut m.texture_path)
                                .desired_width((ui.available_width() - 150.0).max(60.0))
                                .hint_text("roche.png · albedo.jpg"),
                        )
                        .changed();
                    if ui.button("Parcourir").clicked() {
                        self.file_picker.open(
                            file_picker::PickTarget::Texture,
                            file_picker::PickMode::OpenFile,
                            &m.texture_path,
                            &["png", "jpg", "jpeg", "tif", "tiff", "bmp"],
                        );
                    }
                    if ui.button("Effacer").clicked() {
                        m.texture_path.clear();
                        changed = true;
                    }
                });
                changed |= param_f32(
                    ui,
                    "Échelle texture",
                    "m",
                    &mut m.texture_scale_m,
                    0.1,
                    5000.0,
                    80.0,
                );
                changed |= param_f32(
                    ui,
                    "Mélange texture",
                    "",
                    &mut m.texture_mix,
                    0.0,
                    1.0,
                    0.8,
                );
                changed |= param_f32(
                    ui,
                    "Bump texture",
                    "",
                    &mut m.texture_bump,
                    0.0,
                    2.0,
                    0.5,
                );
                changed |= param_f32(
                    ui,
                    "Déplacement texture",
                    "m",
                    &mut m.texture_displacement_m,
                    0.0,
                    30.0,
                    0.0,
                );
                ui.label(
                    egui::RichText::new(
                        "La texture est projetée en XY monde et se répète ; le bump lit sa luminance, le déplacement lit la même valeur en géométrie. Vide = roche procédurale seule.",
                    )
                    .small()
                    .weak(),
                );
                ui.separator();
                changed |= param_f32(
                    ui,
                    "Détail (normale)",
                    "",
                    &mut m.detail_normal,
                    0.0,
                    1.0,
                    0.32,
                );
                changed |= param_f32(
                    ui,
                    "Échelle détail",
                    "m",
                    &mut m.detail_scale_m,
                    1.0,
                    200.0,
                    34.0,
                );
                changed |= param_f32(ui, "Éboulis", "", &mut m.scree, 0.0, 1.0, 0.4);
                changed |= param_f32(ui, "Pente éboulis", "", &mut m.scree_slope, 0.0, 1.0, 0.42);
                changed |= param_f32(ui, "Neige", "", &mut m.snow_coverage, 0.0, 1.0, 0.55);
                changed |= param_f32(ui, "Pente neige", "", &mut m.snow_slope, 0.0, 1.0, 0.62);
                changed |= param_f32(ui, "Végétation", "", &mut m.vegetation, 0.0, 1.0, 0.6);
                changed |= param_f32(ui, "Humidité", "", &mut m.wetness, 0.0, 1.0, 0.3);
                if changed {
                    self.mark_dirty();
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Materials,
        );
        let header = egui::CollapsingHeader::new("Peindre une texture")
            .default_open(false)
            .show(ui, |ui| {
                ui.checkbox(&mut self.paint.open, "Ouvrir le pinceau");
                if !self.paint.open {
                    ui.label(
                        egui::RichText::new(
                            "Peignez une texture de roche, enregistrez-la, et elle sera utilisée par le mélange, le bump et le déplacement de texture.",
                        )
                        .small()
                        .weak(),
                    );
                } else {
                    let mut saved: Option<String> = None;
                    {
                        let paint = &mut self.paint;
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Couleur");
                            ui.color_edit_button_srgb(&mut paint.color);
                            ui.checkbox(&mut paint.eraser, "Gomme");
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Rayon");
                            ui.add(egui::Slider::new(&mut paint.brush_radius, 1.0..=64.0));
                            ui.label("Force");
                            ui.add(egui::Slider::new(&mut paint.strength, 0.05..=1.0));
                        });
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Bruit de roche").clicked() {
                                paint.fill_procedural();
                            }
                            if ui.button("Enregistrer").clicked() {
                                match paint.save(self.project_path.as_deref()) {
                                    Ok(path) => saved = Some(path),
                                    Err(error) => paint.status = error,
                                }
                            }
                        });
                        let size = egui::vec2(ui.available_width().min(300.0), 240.0);
                        let (rect, response) =
                            ui.allocate_exact_size(size, egui::Sense::click_and_drag());
                        if paint.dirty || paint.texture.is_none() {
                            let image = egui::ColorImage::from_rgba_unmultiplied(
                                [paint.width, paint.height],
                                &paint.pixels,
                            );
                            paint.texture = Some(ui.ctx().load_texture(
                                "paint-texture",
                                image,
                                egui::TextureOptions::NEAREST,
                            ));
                            paint.dirty = false;
                        }
                        if let Some(texture) = &paint.texture {
                            ui.painter().image(
                                texture.id(),
                                rect,
                                egui::Rect::from_min_max(
                                    egui::pos2(0.0, 0.0),
                                    egui::pos2(1.0, 1.0),
                                ),
                                egui::Color32::WHITE,
                            );
                        }
                        ui.painter().rect_stroke(
                            rect,
                            0.0,
                            egui::Stroke::new(1.0, egui::Color32::from_gray(110)),
                            egui::StrokeKind::Inside,
                        );
                        if let Some(pointer) = response.hover_pos() {
                            let u = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 0.999);
                            let v = ((pointer.y - rect.top()) / rect.height()).clamp(0.0, 0.999);
                            let px = (u * paint.width as f32) as usize;
                            let py = (v * paint.height as f32) as usize;
                            let down = ui.input(|input| input.pointer.primary_down());
                            if down && response.contains_pointer() {
                                if let Some(last) = paint.last {
                                    paint.paint_line(last, (px, py));
                                } else {
                                    paint.paint(px, py);
                                }
                                paint.last = Some((px, py));
                            } else {
                                paint.last = None;
                            }
                            let radius = paint.brush_radius
                                * rect.width() / paint.width as f32;
                            ui.painter().circle_stroke(
                                pointer,
                                radius,
                                egui::Stroke::new(1.0, egui::Color32::WHITE),
                            );
                        } else {
                            paint.last = None;
                        }
                        ui.label(egui::RichText::new(&paint.status).small().weak());
                    }
                    if let Some(path) = saved {
                        self.project.materials.texture_path = path;
                        self.project.materials.texture_mix = 1.0;
                        self.project.materials.texture_bump = self
                            .project
                            .materials
                            .texture_bump
                            .max(0.35);
                        self.mark_dirty();
                        self.paint.status =
                            "Texture appliquée au matériau (mélange, bump et déplacement)".into();
                        self.start_render(ui.ctx().clone(), false);
                    }
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Materials,
        );

        let header = egui::CollapsingHeader::new("Étagement")
            .default_open(false)
            .show(ui, |ui| {
                let stats = self.terrain_stats();
                let (min, max) = (stats.min, stats.max);
                let mut changed = false;
                changed |= param_f32(
                    ui,
                    "Ligne de neige",
                    "m",
                    &mut self.project.style.snow_line_m,
                    min,
                    max,
                    1700.0,
                );
                changed |= param_f32(
                    ui,
                    "Limite des arbres",
                    "m",
                    &mut self.project.style.tree_line_m,
                    min,
                    max,
                    1300.0,
                );
                changed |= param_f32(
                    ui,
                    "Plage",
                    "m",
                    &mut self.project.style.beach_height_m,
                    0.0,
                    200.0,
                    8.0,
                );
                changed |= param_f32(
                    ui,
                    "Seuil roche",
                    "",
                    &mut self.project.style.rock_slope,
                    0.0,
                    1.0,
                    0.55,
                );
                changed |= param_f32(
                    ui,
                    "Seuil neige",
                    "",
                    &mut self.project.style.snow_slope,
                    0.0,
                    1.0,
                    0.62,
                );
                if changed {
                    self.mark_dirty();
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Materials,
        );
    }

    fn erosion_ui(&mut self, ui: &mut egui::Ui) {
        let header = egui::CollapsingHeader::new("Érosion")
            .default_open(true)
            .show(ui, |ui| {
                ui.checkbox(&mut self.project.erosion_enabled, "Érosion activée");
                let e = &mut self.project.erosion;
                let mut changed = false;
                ui.horizontal(|ui| {
                    changed |= ui.checkbox(&mut e.hydraulic, "Hydraulique").changed();
                    changed |= ui.checkbox(&mut e.thermal, "Thermique").changed();
                });
                changed |= param_u32(ui, "Gouttes", "", &mut e.droplets, 0, 2_000_000, 50_000);
                changed |= param_f32(
                    ui,
                    "Taux d'érosion",
                    "",
                    &mut e.erosion_rate,
                    0.0,
                    1.0,
                    0.06,
                );
                changed |= param_f32(ui, "Dépôt", "", &mut e.deposition_rate, 0.0, 1.0, 0.12);
                changed |= param_f32(ui, "Inertie", "", &mut e.inertia, 0.0, 1.0, 0.05);
                changed |= param_f32(ui, "Capacité", "", &mut e.capacity, 0.0, 5.0, 1.2);
                changed |= param_f32(ui, "Évaporation", "", &mut e.evaporation, 0.0, 1.0, 0.015);
                changed |= param_f32(ui, "Gravité", "", &mut e.gravity, 0.0, 10.0, 4.0);
                changed |= param_f32(
                    ui,
                    "Angle de talus",
                    "°",
                    &mut e.talus_angle_deg,
                    5.0,
                    60.0,
                    38.0,
                );
                changed |= param_u32(
                    ui,
                    "Itérations talus",
                    "",
                    &mut e.thermal_iterations,
                    0,
                    100,
                    12,
                );
                if changed {
                    self.mark_dirty();
                }
                if ui
                    .add_sized(
                        [ui.available_width(), 26.0],
                        egui::Button::new("Régénérer avec cette érosion"),
                    )
                    .clicked()
                {
                    self.generate_terrain(ui.ctx().clone());
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Erosion,
        );
    }

    fn ensure_animation_workspace(&mut self, ctx: &egui::Context) {
        if animation_ui::ensure_animation(&mut self.project.rig, &self.heightfield) {
            self.project.mark_modified();
            self.anim_revision = self.anim_revision.wrapping_add(1);
            self.anim_camera_dirty = true;
            self.rig_status = "Tracé de démonstration chargé : modifiez-le ou cliquez Lire.".into();
            let length = vf_flight::rig::path_length(&self.project.rig.eye);
            if length < 3000.0 {
                // A fresh demo path is small; frame it so the first open is
                // immediately readable instead of showing an invisible speck.
                self.frame_animation_loop();
                self.anim_framed = true;
            }
        }
        if !self.anim_framed {
            self.anim_framed = true;
            let length = vf_flight::rig::path_length(&self.project.rig.eye);
            // A saved project may already contain a small authored path but a
            // design camera framed on the whole terrain. Frame the path once
            // when the workspace is opened, then leave the camera alone.
            if length > 1.0 && length < 3000.0 {
                self.frame_animation_loop();
            }
        }
        self.anim_editor.clear_invalid_selection(&self.project.rig);
        self.rig_time = self
            .rig_time
            .clamp(0.0, self.project.rig.duration_s.max(0.1));
        // While a key is being dragged, rebuilding the track on every mouse
        // move would simulate the whole clip at 60 Hz. Keep the previous
        // sample and rebuild once the drag ends.
        if !self.anim_editor.is_dragging()
            && !self.anim_timeline.is_dragging()
            && self.anim_camera_dirty
        {
            if self.anim_view == animation_ui::AnimViewMode::Camera {
                self.sample_animation_camera(ctx);
            } else if let Some(sample) = self.animation_sample_at(self.rig_time) {
                // In design mode this keeps the frustum and target overlays in
                // sync with the safety layer without stealing the orbit camera.
                self.anim_live_sample = Some(sample);
                self.anim_camera_dirty = false;
            }
        }
    }

    fn mark_animation_changed(&mut self) {
        self.history.label("Animation");
        self.anim_revision = self.anim_revision.wrapping_add(1);
        self.anim_profile_cache = None;
        self.anim_compare_cache = None;
        self.anim_camera_dirty = true;
        self.anim_live_sample = None;
        self.anim_track_error = None;
        self.project.mark_modified();
    }

    /// Return the precomputed animation track for the current rig revision.
    /// Building it is O(duration) and is therefore cached; sampling is O(1).
    fn animation_track(&mut self) -> Option<&vf_flight::rig::RigTrack> {
        let stale = self
            .anim_track
            .as_ref()
            .is_none_or(|(revision, _)| *revision != self.anim_revision);
        if stale {
            match vf_flight::rig::RigTrack::new(
                &self.project.rig,
                &self.heightfield,
                self.project.seed,
            ) {
                Ok(track) => self.anim_track = Some((self.anim_revision, track)),
                Err(error) => {
                    self.rig_status = format!("Animation : {error}");
                    self.anim_track = None;
                    return None;
                }
            }
        }
        self.anim_track.as_ref().map(|(_, track)| track)
    }

    fn animation_sample_at(&mut self, time: f32) -> Option<vf_flight::rig::RigSample> {
        self.animation_track().map(|track| track.sample_at(time))
    }

    /// Recompute the worst distance between the simulated path and the
    /// authored curve. The track is cached, so this is cheap after the first
    /// call for a given revision.
    fn refresh_tracking_error(&mut self) {
        if self.anim_track_error.is_some() {
            return;
        }
        let path = self.project.rig.eye.clone();
        let error = self
            .animation_track()
            .map(|track| track.max_path_error(&path));
        self.anim_track_error = error;
    }

    /// Make the selected path use constant-speed segments (ease 0). Per-key
    /// smoothstep makes the camera slow down and surge at every key, which is
    /// often read as "the camera does not follow the curve".
    fn flatten_key_easing(&mut self) {
        let mut changed = false;
        for key in &mut self.project.rig.eye.keys {
            if key.ease.abs() > 1e-6 {
                key.ease = 0.0;
                changed = true;
            }
        }
        for key in &mut self.project.rig.target.path.keys {
            if key.ease.abs() > 1e-6 {
                key.ease = 0.0;
                changed = true;
            }
        }
        if changed {
            self.mark_animation_changed();
            self.rig_status = "Adoucissement remis à 0 : vitesse constante entre les clés".into();
        }
    }

    fn sample_animation_camera(&mut self, ctx: &egui::Context) -> bool {
        let sampled = self.animation_sample_at(self.rig_time);
        match sampled {
            Some(sample) => {
                self.project.camera = sample.camera.clone();
                self.sync_orbit_from_camera();
                self.rig_status = format!(
                    "t={:.2}s · {:.0} m sol · gîte {:.1}° · vitesse {:.1} m/s{}",
                    self.rig_time,
                    sample.altitude_agl_m,
                    sample.bank_deg,
                    sample.speed_mps,
                    safety_label(&sample.safety)
                );
                self.anim_live_sample = Some(sample);
                self.anim_camera_dirty = false;
                self.refresh_camera_preview(ctx);
                true
            }
            None => false,
        }
    }

    fn enter_animation_camera(&mut self, ctx: &egui::Context) {
        if self.anim_view == animation_ui::AnimViewMode::Design {
            self.anim_design_camera = Some(self.project.camera.clone());
        }
        self.anim_view = animation_ui::AnimViewMode::Camera;
        self.sample_animation_camera(ctx);
    }

    fn enter_animation_design(&mut self, ctx: &egui::Context) {
        self.anim_playing = false;
        if let Some(camera) = self.anim_design_camera.take() {
            self.project.camera = camera;
            self.sync_orbit_from_camera();
        }
        self.anim_view = animation_ui::AnimViewMode::Design;
        self.anim_editor.trace = false;
        self.mark_camera_dirty();
        ctx.request_repaint();
    }

    fn toggle_animation_play(&mut self, ctx: &egui::Context) {
        if self.anim_playing {
            self.anim_playing = false;
            if self.anim_view == animation_ui::AnimViewMode::Camera {
                self.refresh_camera_preview(ctx);
            }
            return;
        }
        let duration = self.project.rig.duration_s.max(0.1);
        if self.rig_time >= duration - 1e-4 {
            self.rig_time = 0.0;
        }
        // Playback follows the current view. Design playback animates the
        // frustum/markers without stealing the orbit camera; camera playback
        // shows the rendered shot. This is the behaviour the transport bar
        // promises (Design / Caméra are view modes, not playback modes).
        if self.anim_view == animation_ui::AnimViewMode::Camera {
            if let Some(sample) = self.animation_sample_at(self.rig_time) {
                self.project.camera = sample.camera.clone();
                self.sync_orbit_from_camera();
                self.anim_live_sample = Some(sample);
            }
        } else if let Some(sample) = self.animation_sample_at(self.rig_time) {
            self.anim_live_sample = Some(sample);
        }
        self.anim_playing = true;
        self.anim_last_play_preview = Instant::now() - Duration::from_secs(1);
        ctx.request_repaint();
    }

    fn pump_animation(&mut self, ctx: &egui::Context) {
        if !self.anim_playing || self.workspace != Workspace::Animation {
            return;
        }
        let duration = self.project.rig.duration_s.max(0.1);
        let dt = ctx.input(|input| input.stable_dt).min(0.1) * self.anim_play_speed.max(0.05);
        self.rig_time += dt;
        if self.rig_time >= duration {
            if self.anim_loop {
                self.rig_time %= duration;
            } else {
                self.rig_time = duration;
                self.anim_playing = false;
            }
        }
        if let Some(sample) = self.animation_sample_at(self.rig_time) {
            let camera_view = self.anim_view == animation_ui::AnimViewMode::Camera;
            if camera_view {
                self.project.camera = sample.camera.clone();
                self.rig_status = format!(
                    "Lecture caméra t={:.2}s · {:.0} m sol · gîte {:.1}° · {:.1} m/s{}",
                    self.rig_time,
                    sample.altitude_agl_m,
                    sample.bank_deg,
                    sample.speed_mps,
                    safety_label(&sample.safety)
                );
            } else {
                self.rig_status = format!(
                    "Lecture design t={:.2}s · caméra {:.0}, {:.0}, {:.0} · {:.1} m/s{}",
                    self.rig_time,
                    sample.eye.x,
                    sample.eye.y,
                    sample.eye.z,
                    sample.speed_mps,
                    safety_label(&sample.safety)
                );
            }
            self.anim_live_sample = Some(sample);
        } else {
            self.anim_playing = false;
        }
        let camera_view = self.anim_view == animation_ui::AnimViewMode::Camera;
        if self.gpu_enabled && self.gpu.is_some() {
            ctx.request_repaint();
        } else if camera_view
            && self.render_task.is_none()
            && self.anim_last_play_preview.elapsed() >= Duration::from_millis(90)
        {
            self.preview_scale = self.interactive_scale;
            self.mark_camera_dirty();
            self.start_render(ctx.clone(), false);
            self.anim_last_play_preview = Instant::now();
        }
        if self.anim_playing {
            ctx.request_repaint_after(Duration::from_millis(16));
        } else if camera_view {
            self.refresh_camera_preview(ctx);
        }
    }

    /// Persistent transport bar at the bottom of the viewport. The animation
    /// is always one click away, whatever tab is open: this is the missing
    /// "play" affordance of the old split Flight/Animation workspaces.
    fn animation_transport_ui(&mut self, ctx: &egui::Context, viewport: egui::Rect) {
        if viewport.width() < 420.0 || viewport.height() < 220.0 {
            self.anim_transport_rect = egui::Rect::NOTHING;
            return;
        }
        let duration = self.project.rig.duration_s.max(0.1);
        let width = (viewport.width() - 28.0).clamp(360.0, 1180.0);
        let area = egui::Area::new(egui::Id::unique("vf-anim-transport"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::Pos2::new(
                viewport.center().x,
                viewport.bottom() - 10.0,
            ))
            .pivot(egui::Align2::CENTER_BOTTOM)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(egui::Color32::from_rgba_unmultiplied(12, 16, 23, 238))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(54, 74, 96)))
                    .corner_radius(9.0)
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .add_sized([48.0, 24.0], egui::Button::new("Début").small())
                                .on_hover_text("Revenir au début (t = 0)")
                                .clicked()
                            {
                                self.anim_playing = false;
                                self.rig_time = 0.0;
                                self.anim_camera_dirty = true;
                                self.anim_live_sample = None;
                                if self.anim_view == animation_ui::AnimViewMode::Camera {
                                    self.sample_animation_camera(ctx);
                                }
                            }
                            let play_label = if self.anim_playing { "Pause" } else { "Lire" };
                            if ui
                                .add_sized(
                                    [78.0, 24.0],
                                    egui::Button::new(
                                        egui::RichText::new(play_label).strong().size(12.0),
                                    )
                                    .fill(
                                        if self.anim_playing {
                                            egui::Color32::from_rgb(74, 48, 48)
                                        } else {
                                            egui::Color32::from_rgb(38, 82, 62)
                                        },
                                    ),
                                )
                                .on_hover_text("Lancer / mettre en pause (Espace)")
                                .clicked()
                            {
                                self.toggle_animation_play(ctx);
                            }
                            let fps = self.project.rig.fps.max(1) as f32;
                            let mut time = self.rig_time.clamp(0.0, duration);
                            let slider_width = (ui.available_width() - 320.0).clamp(120.0, 620.0);
                            let slider = ui.add_sized(
                                [slider_width, 20.0],
                                egui::Slider::new(&mut time, 0.0..=duration)
                                    .show_value(false)
                                    .trailing_fill(true),
                            );
                            if slider.changed() {
                                self.anim_playing = false;
                                self.rig_time = time;
                                self.anim_camera_dirty = true;
                                self.anim_live_sample = None;
                                if self.anim_view == animation_ui::AnimViewMode::Camera {
                                    self.sample_animation_camera(ctx);
                                }
                                ctx.request_repaint();
                            }
                            ui.label(
                                egui::RichText::new(format!("{time:5.2}s"))
                                    .monospace()
                                    .size(11.0),
                            );
                            if ui
                                .add_sized([26.0, 22.0], egui::Button::new("−").small())
                                .on_hover_text("Image précédente")
                                .clicked()
                            {
                                self.anim_playing = false;
                                self.rig_time = (self.rig_time - 1.0 / fps).max(0.0);
                                self.anim_live_sample = None;
                                if self.anim_view == animation_ui::AnimViewMode::Camera {
                                    self.sample_animation_camera(ctx);
                                }
                            }
                            if ui
                                .add_sized([26.0, 22.0], egui::Button::new("+").small())
                                .on_hover_text("Image suivante")
                                .clicked()
                            {
                                self.anim_playing = false;
                                self.rig_time = (self.rig_time + 1.0 / fps).min(duration);
                                self.anim_live_sample = None;
                                if self.anim_view == animation_ui::AnimViewMode::Camera {
                                    self.sample_animation_camera(ctx);
                                }
                            }
                            if ui
                                .selectable_label(self.anim_loop, "Boucle")
                                .on_hover_text("Lecture en boucle")
                                .clicked()
                            {
                                self.anim_loop = !self.anim_loop;
                            }
                            ui.separator();
                            if ui
                                .selectable_label(
                                    self.anim_view == animation_ui::AnimViewMode::Design,
                                    "Design",
                                )
                                .on_hover_text("Voir et éditer le tracé")
                                .clicked()
                            {
                                self.enter_animation_design(ctx);
                            }
                            if ui
                                .selectable_label(
                                    self.anim_view == animation_ui::AnimViewMode::Camera,
                                    "Caméra",
                                )
                                .on_hover_text("Voir ce que la caméra filme")
                                .clicked()
                            {
                                self.enter_animation_camera(ctx);
                            }
                            if ui
                                .selectable_label(self.anim_timeline.open, "Timeline")
                                .on_hover_text("Afficher/masquer la timeline et le graph editor")
                                .clicked()
                            {
                                self.anim_timeline.open = !self.anim_timeline.open;
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} images · {:.2}s · {} fps · profil {}{}",
                                    self.project.rig.frame_count(),
                                    duration,
                                    self.project.rig.fps,
                                    self.project.rig.body.preset.label(),
                                    if self.anim_playing {
                                        if self.anim_view == animation_ui::AnimViewMode::Design {
                                            " · lecture design"
                                        } else {
                                            " · lecture caméra"
                                        }
                                    } else {
                                        ""
                                    }
                                ))
                                .size(9.5)
                                .color(egui::Color32::from_rgb(150, 168, 186)),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new("Espace : lire/pause · F1 : aide")
                                            .size(9.5)
                                            .color(egui::Color32::from_rgb(122, 140, 160)),
                                    );
                                },
                            );
                        });
                    });
            });
        self.anim_transport_rect = area.response.rect;
    }

    /// Development-only documentation capture. See `CaptureState`.
    fn pump_capture(&mut self, ctx: &egui::Context) {
        let Some(cap) = self.capture.as_ref() else {
            return;
        };
        let index = cap.index;
        let steps = cap.steps.len();
        if index >= steps {
            self.capture = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        ctx.request_repaint();
        if cap.requested {
            let wait = cap.wait_after_request;
            if wait >= 4 {
                if let Some(cap) = self.capture.as_mut() {
                    cap.index += 1;
                    cap.frame = 0;
                    cap.requested = false;
                    cap.wait_after_request = 0;
                }
            } else if let Some(cap) = self.capture.as_mut() {
                cap.wait_after_request += 1;
            }
            return;
        }
        let frame = cap.frame;
        if frame == 0 {
            let step = cap.steps[index];
            self.capture_apply_step(step, ctx);
        }
        if let Some(cap) = self.capture.as_mut() {
            cap.frame += 1;
        }
        let step = self.capture.as_ref().map(|cap| cap.steps[cap.index]);
        let dir = self.capture.as_ref().map(|cap| cap.dir.clone());
        let (Some(step), Some(dir)) = (step, dir) else {
            return;
        };
        if step.name == "workspace_compare" {
            if self.capture_compare_stage == 0 {
                if self.compare.before.is_some() {
                    // The baseline is frozen; now make a visible change for
                    // the right side of the split.
                    self.capture_compare_stage = 1;
                    self.project.materials.rock_preset = "basalt".into();
                    self.project.materials.bump_strength = 1.1;
                    self.mark_dirty();
                }
                return;
            }
            if self.capture_compare_stage == 1 {
                return;
            }
        }
        if frame < 28
            || self.render_task.is_some()
            || self.task.is_some()
            || self.hillshade_task.is_some()
            || self.gpu_mesh_task.is_some()
        {
            return;
        }
        let path = dir.join(format!("{}.png", step.name));
        if let Some(cap) = self.capture.as_mut() {
            cap.requested = true;
            cap.wait_after_request = 0;
        }
        ctx.request_screenshot(move |image| save_capture_image(&path, &image));
    }

    fn capture_apply_step(&mut self, step: CaptureStep, ctx: &egui::Context) {
        self.workspace = step.workspace;
        if step.name == "workspace_compare" {
            // Start comparison and let the baseline render finish before the
            // capture mutates the material (see `pump_capture`).
            self.cpu_preview_forced = true;
            self.capture_compare_stage = 0;
            self.compare_start(ctx);
        }
        self.preferences_open = step.preferences;
        self.anim_tab = step.anim_tab;
        self.anim_compare = step.compare;
        self.anim_compare_cache = None;
        match step.help {
            Some(topic) => self.help.reveal(topic),
            None => self.help.close(),
        }
        if step.workspace == Workspace::Animation {
            self.ensure_animation_workspace(ctx);
            if step.compare && animation_ui::TimingFit::measure(&self.project.rig).is_strained() {
                let _ =
                    vf_flight::rig::preset_path(&mut self.project.rig, &self.heightfield, "demo");
                self.rig_time = 0.0;
                self.anim_editor.selection = None;
                self.frame_animation_loop();
                self.anim_revision = self.anim_revision.wrapping_add(1);
                self.anim_profile_cache = None;
                self.anim_compare_cache = None;
            }
            if step.design {
                if self.anim_view != animation_ui::AnimViewMode::Design {
                    self.enter_animation_design(ctx);
                }
                self.anim_editor.trace = false;
                if step.select_key
                    && let Some(key) = self.project.rig.eye.keys.get(1).cloned()
                {
                    self.rig_time = key.time_s;
                    self.anim_editor.selection = Some(animation_ui::AnimSelection::EyeKey(1));
                    self.anim_edit_target = false;
                } else if !step.select_key {
                    self.anim_editor.selection = None;
                    self.anim_edit_target = false;
                }
            } else {
                self.enter_animation_camera(ctx);
            }
            self.mark_camera_dirty();
        }
        ctx.request_repaint();
    }

    fn animation_panel_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.ensure_animation_workspace(ctx);
        self.help.register_front(
            ui.available_rect_before_wrap(),
            help_topic_for_tab(self.anim_tab),
        );
        ui.horizontal_wrapped(|ui| {
            for tab in animation_ui::AnimTab::ALL {
                let response = ui.selectable_label(self.anim_tab == tab, tab.short());
                if response.hovered() {
                    self.help.register(response.rect, help_topic_for_tab(tab));
                }
                if response.clicked() {
                    self.anim_tab = tab;
                }
            }
        });
        ui.separator();
        if !self.anim_editor.is_dragging() && !self.anim_timeline.is_dragging() {
            self.refresh_tracking_error();
        }
        if let Some(error) = self.anim_track_error {
            let fit = animation_ui::TimingFit::measure(&self.project.rig);
            let threshold = (fit.length_m as f32 * 0.01).max(5.0);
            if error > threshold {
                egui::Frame::group(ui.style())
                    .fill(egui::Color32::from_rgba_unmultiplied(58, 40, 18, 180))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "Caméra jusqu'à {error:.0} m derrière le tracé : le profil coupe les virages ou sature (seuil {threshold:.0} m)."
                            ))
                            .size(10.5)
                            .color(egui::Color32::from_rgb(255, 196, 110)),
                        );
                        ui.horizontal_wrapped(|ui| {
                            if ui.button("Allonger la durée").clicked() {
                                let recommended = animation_ui::TimingFit::measure(&self.project.rig)
                                    .recommended_duration_s
                                    .max(self.project.rig.duration_s * 1.15);
                                self.set_animation_duration(recommended);
                            }
                            if ui
                                .button("Vitesse constante (ease 0)")
                                .on_hover_text(
                                    "Supprime le ralentissement/accélération à chaque clé ; c'est souvent le vrai correctif.",
                                )
                                .clicked()
                            {
                                self.flatten_key_easing();
                            }
                            if ui.button("Profil manuel").clicked() {
                                animation_ui::apply_aircraft_preset(
                                    &mut self.project.rig,
                                    AircraftPreset::Manual,
                                );
                                self.mark_animation_changed();
                            }
                        });
                    });
            }
        }
        match self.anim_tab {
            animation_ui::AnimTab::Path => self.anim_path_tab(ui, ctx),
            animation_ui::AnimTab::Timing => self.anim_timing_tab(ui, ctx),
            animation_ui::AnimTab::Profile => self.anim_profile_tab(ui, ctx),
            animation_ui::AnimTab::Preview => self.anim_preview_tab(ui, ctx),
        }
        if self.anim_view == animation_ui::AnimViewMode::Camera
            && !self.anim_playing
            && self.anim_camera_dirty
        {
            self.sample_animation_camera(ctx);
        }
        ui.separator();
        ui.label(egui::RichText::new(&self.rig_status).small().weak());
    }

    fn anim_path_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(
            egui::RichText::new(
                "Étape 1 · Dessinez le trajet de la caméra, puis matérialisez éventuellement la cible. Le gizmo X/Y/Z déplace la clé sélectionnée.",
            )
            .small(),
        );
        ui.horizontal_wrapped(|ui| {
            if ui
                .selectable_label(
                    self.anim_view == animation_ui::AnimViewMode::Design,
                    "Vue design",
                )
                .clicked()
            {
                self.enter_animation_design(ctx);
            }
            if ui
                .selectable_label(
                    self.anim_view == animation_ui::AnimViewMode::Camera,
                    "Vue caméra",
                )
                .clicked()
            {
                self.enter_animation_camera(ctx);
            }
            if ui.selectable_label(self.anim_playing, "Lecture").clicked() {
                self.toggle_animation_play(ctx);
            }
        });
        ui.horizontal_wrapped(|ui| {
            let selection_button = ui.selectable_label(!self.anim_editor.trace, "Sélection");
            if selection_button.hovered() {
                self.help
                    .register(selection_button.rect, help::HelpTopic::Path);
            }
            if selection_button.clicked() {
                self.anim_editor.trace = false;
            }
            let trace_button = ui.selectable_label(self.anim_editor.trace, "Tracer");
            if trace_button.hovered() {
                self.help.register(trace_button.rect, help::HelpTopic::Path);
            }
            if trace_button.clicked() {
                self.anim_editor.trace = true;
                self.anim_view = animation_ui::AnimViewMode::Design;
                self.anim_playing = false;
            }
            ui.checkbox(&mut self.anim_editor.show_paths, "Tracés");
            ui.checkbox(&mut self.anim_editor.show_trail, "Trajectoire simulée");
        });
        if self.anim_editor.trace {
            ui.horizontal_wrapped(|ui| {
                ui.label("Ajouter");
                ui.selectable_value(&mut self.anim_editor.trace_target, false, "Caméra");
                ui.selectable_value(&mut self.anim_editor.trace_target, true, "Cible");
                ui.label("altitude");
                ui.add(
                    egui::DragValue::new(&mut self.anim_editor.trace_altitude)
                        .speed(2.0)
                        .range(0.0..=3000.0)
                        .suffix(" m"),
                );
            });
            ui.label(
                egui::RichText::new(
                    "Cliquez sur le terrain pour ajouter un point. Échap ou Sélection pour quitter.",
                )
                .small()
                .weak(),
            );
        }
        ui.horizontal_wrapped(|ui| {
            ui.label("Aimant");
            ui.add(
                egui::DragValue::new(&mut self.anim_editor.snap_m)
                    .speed(1.0)
                    .range(0.0..=1000.0)
                    .suffix(" m"),
            );
            ui.label(egui::RichText::new("0 = libre").small().weak());
            ui.checkbox(&mut self.anim_editor.free_drag, "Glisser libre")
                .on_hover_text(
                    "Cliquer-glisser directement une clé la déplace dans le plan de l'écran. Maj = vertical, Alt = poser sur le terrain.",
                );
        });
        ui.label(
            egui::RichText::new(
                "Trois méthodes équivalentes : gizmo X/Y/Z, glisser libre dans le viewport, ou valeurs numériques ci-dessous.",
            )
            .small()
            .weak(),
        );
        ui.separator();

        // Target mode.
        let previous_mode = self.project.rig.target.mode;
        let mut mode = previous_mode;
        ui.horizontal(|ui| {
            let label = ui.label("Point de vue");
            if label.hovered() {
                self.help.register(label.rect, help::HelpTopic::Target);
            }
            let combo = egui::ComboBox::from_id_salt("anim-target-mode")
                .selected_text(target_mode_label(mode))
                .show_ui(ui, |ui| {
                    for candidate in [
                        TargetMode::Fixed,
                        TargetMode::EyeAhead,
                        TargetMode::None,
                        TargetMode::Offset,
                        TargetMode::Trail,
                        TargetMode::Path,
                        TargetMode::Manual,
                    ] {
                        ui.selectable_value(&mut mode, candidate, target_mode_label(candidate));
                    }
                });
            if combo.response.hovered() {
                self.help
                    .register(combo.response.rect, help::HelpTopic::Target);
            }
        });
        if self.project.rig.exact_path {
            ui.label(
                egui::RichText::new(
                    "Suivi exact actif : la visée est verrouillée sur la courbe, le lissage de visée est ignoré. Décochez « Suivre exactement le tracé » pour l'utiliser.",
                )
                .small()
                .weak(),
            );
        }
        if mode != previous_mode {
            self.project.rig.target.mode = mode;
            if mode == TargetMode::Path {
                self.ensure_target_path();
                self.anim_edit_target = true;
            }
            if mode == TargetMode::Fixed {
                self.anim_edit_target = false;
                if self.project.rig.target.fixed == [0.0; 3] {
                    self.project.rig.target.fixed = self.terrain_center_point();
                }
            }
            if mode == TargetMode::Offset {
                self.project.rig.target.smoothing = 0.0;
                if self.project.rig.target.offset == [0.0; 3] {
                    self.project.rig.target.offset = [0.0, 350.0, -80.0];
                }
            }
            if mode == TargetMode::Trail && self.project.rig.target.lead_s < 0.05 {
                self.project.rig.target.lead_s = 0.8;
            }
            self.anim_edit_target = false;
            self.anim_editor.selection = None;
            self.mark_animation_changed();
        }
        match self.project.rig.target.mode {
            TargetMode::Fixed => {
                let mut changed = false;
                ui.horizontal_wrapped(|ui| {
                    ui.label("Position");
                    for axis in 0..3 {
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut self.project.rig.target.fixed[axis])
                                    .speed(10.0),
                            )
                            .changed();
                    }
                    if ui.button("Centre du terrain").clicked() {
                        self.project.rig.target.fixed = self.terrain_center_point();
                        changed = true;
                    }
                });
                if changed {
                    self.mark_animation_changed();
                    self.anim_editor.selection = Some(animation_ui::AnimSelection::FixedTarget);
                }
            }
            TargetMode::Path => {
                ui.label(
                    egui::RichText::new(
                        "La cible suit son propre tracé, indépendant de la caméra. Sélectionnez-la puis cliquez sur une de ses clés.",
                    )
                    .small()
                    .weak(),
                );
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button("Copier le tracé caméra")
                        .on_hover_text("Duplique les clés de la caméra, sans décalage.")
                        .clicked()
                    {
                        self.copy_eye_path_to_target([0.0; 3]);
                    }
                    if ui
                        .button("Copier + décalage")
                        .on_hover_text(
                            "Duplique les clés de la caméra en ajoutant le décalage de la cible.",
                        )
                        .clicked()
                    {
                        let offset = self.project.rig.target.offset;
                        self.copy_eye_path_to_target(offset);
                    }
                    if self.project.rig.target.path.keys.len() == self.project.rig.eye.keys.len()
                        && ui
                            .button("Synchroniser les temps")
                            .on_hover_text("Copie les temps des clés de la caméra sur la cible.")
                            .clicked()
                        && self.sync_target_times()
                    {
                        self.mark_animation_changed();
                        self.rig_status = "Temps de la cible synchronisés sur la caméra".into();
                    }
                });
                let mut changed = false;
                ui.horizontal_wrapped(|ui| {
                    ui.label("Décalage");
                    for axis in 0..3 {
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut self.project.rig.target.offset[axis])
                                    .speed(10.0)
                                    .prefix(["X ", "Y ", "Z "][axis]),
                            )
                            .changed();
                    }
                });
                if changed {
                    self.mark_animation_changed();
                }
            }
            TargetMode::EyeAhead => {
                let mut changed = param_f32(
                    ui,
                    "Avance sur la courbe",
                    "s",
                    &mut self.project.rig.target.lead_s,
                    -5.0,
                    5.0,
                    0.35,
                );
                if changed {
                    self.mark_animation_changed();
                }
                changed = param_f32(
                    ui,
                    "Lissage visée",
                    "",
                    &mut self.project.rig.target.smoothing,
                    0.0,
                    1.0,
                    0.35,
                );
                if changed {
                    self.mark_animation_changed();
                }
            }
            TargetMode::Offset => {
                ui.label(
                    egui::RichText::new(
                        "Liaison rigide : la cible reste à un décalage monde constant de la caméra.",
                    )
                    .small()
                    .weak(),
                );
                let mut changed = false;
                ui.horizontal_wrapped(|ui| {
                    ui.label("Décalage");
                    for axis in 0..3 {
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut self.project.rig.target.offset[axis])
                                    .speed(10.0)
                                    .prefix(["X ", "Y ", "Z "][axis]),
                            )
                            .changed();
                    }
                });
                changed |= param_f32(
                    ui,
                    "Lissage visée",
                    "",
                    &mut self.project.rig.target.smoothing,
                    0.0,
                    1.0,
                    0.2,
                );
                if changed {
                    self.mark_animation_changed();
                }
            }
            TargetMode::Trail => {
                ui.label(
                    egui::RichText::new(
                        "Regard arrière : la cible vise la caméra telle qu'elle était il y a N secondes, sur la courbe.",
                    )
                    .small()
                    .weak(),
                );
                let mut changed = param_f32(
                    ui,
                    "Retard",
                    "s",
                    &mut self.project.rig.target.lead_s,
                    0.0,
                    5.0,
                    0.8,
                );
                ui.horizontal_wrapped(|ui| {
                    ui.label("Décalage");
                    for axis in 0..3 {
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut self.project.rig.target.offset[axis])
                                    .speed(10.0)
                                    .prefix(["X ", "Y ", "Z "][axis]),
                            )
                            .changed();
                    }
                });
                changed |= param_f32(
                    ui,
                    "Lissage visée",
                    "",
                    &mut self.project.rig.target.smoothing,
                    0.0,
                    1.0,
                    0.25,
                );
                if changed {
                    self.mark_animation_changed();
                }
            }
            TargetMode::Manual => {
                ui.label(
                    egui::RichText::new("Orientation manuelle : la caméra suit son axe.")
                        .small()
                        .weak(),
                );
            }
            TargetMode::None => {
                ui.label(
                    egui::RichText::new(
                        "Aucune cible : la caméra regarde devant elle, le long de la trajectoire. Il n'y a plus de point cible à keyframer ni à afficher.",
                    )
                    .small()
                    .weak(),
                );
            }
        }

        ui.separator();
        ui.horizontal_wrapped(|ui| {
            for (name, label) in [("orbit", "Orbite"), ("ridge", "Crête"), ("dive", "Piqué")] {
                if ui.button(label).clicked() {
                    match vf_flight::rig::preset_path(
                        &mut self.project.rig,
                        &self.heightfield,
                        name,
                    ) {
                        Ok(()) => {
                            self.anim_editor.selection = None;
                            self.anim_edit_target = false;
                            self.rig_time = 0.0;
                            self.mark_animation_changed();
                            self.rig_status = format!("Tracé {label} généré");
                        }
                        Err(error) => self.rig_status = error,
                    }
                }
            }
            if ui.button("Durée = fin du tracé").clicked() {
                let end = self
                    .project
                    .rig
                    .eye
                    .keys
                    .last()
                    .map_or(8.0, |key| key.time_s.max(0.1));
                self.set_animation_duration(end);
            }
            if ui
                .button("Cadrer le tracé")
                .on_hover_text("Replace la caméra de conception au centre du tracé")
                .clicked()
            {
                self.frame_animation_loop();
            }
            if ui
                .button("Vitesse constante (ease 0)")
                .on_hover_text(
                    "Supprime l'adoucissement par clé : la caméra ne ralentit plus à chaque point.",
                )
                .clicked()
            {
                self.flatten_key_easing();
            }
            let recommended = animation_ui::TimingFit::measure(&self.project.rig);
            if ui
                .button(format!(
                    "Durée conseillée ({:.0} s)",
                    recommended.recommended_duration_s
                ))
                .on_hover_text(
                    "Durée à laquelle le profil de vol peut suivre le tracé sans saturer",
                )
                .clicked()
            {
                self.set_animation_duration(recommended.recommended_duration_s);
                self.rig_status = format!(
                    "Durée ajustée à {:.1}s pour ce profil · clés étirées",
                    self.project.rig.duration_s
                );
            }
        });

        ui.separator();
        ui.horizontal_wrapped(|ui| {
            if ui
                .selectable_label(!self.anim_edit_target, "Clés caméra")
                .clicked()
            {
                self.anim_edit_target = false;
                self.anim_editor.selection = None;
            }
            let target_editable = self.project.rig.target.mode == TargetMode::Path;
            let target_keys = ui.add_enabled(
                target_editable,
                egui::Button::selectable(self.anim_edit_target, "Clés de la cible"),
            );
            if target_keys.hovered() {
                self.help
                    .register(target_keys.rect, help::HelpTopic::Target);
            }
            if target_keys.clicked() {
                self.anim_edit_target = true;
                self.anim_editor.selection = None;
            }
            if !target_editable {
                ui.label(egui::RichText::new("(cible non animée)").small().weak());
            }
        });

        let editing_target =
            self.anim_edit_target && self.project.rig.target.mode == TargetMode::Path;
        let keys: Vec<(usize, f32, [f64; 3])> = if editing_target {
            self.project
                .rig
                .target
                .path
                .keys
                .iter()
                .enumerate()
                .map(|(index, key)| (index, key.time_s, key.position))
                .collect()
        } else {
            self.project
                .rig
                .eye
                .keys
                .iter()
                .enumerate()
                .map(|(index, key)| (index, key.time_s, key.position))
                .collect()
        };
        let mut clicked_key = None;
        for (index, time, position) in &keys {
            let selection = if editing_target {
                animation_ui::AnimSelection::TargetKey(*index)
            } else {
                animation_ui::AnimSelection::EyeKey(*index)
            };
            let selected = self.anim_editor.selection == Some(selection);
            let label = format!(
                "{} {} · t={:.2}s · ({:.0}, {:.0}, {:.0})",
                if editing_target { "T" } else { "E" },
                index,
                time,
                position[0],
                position[1],
                position[2]
            );
            let response = ui.selectable_label(selected, label);
            if response.hovered() {
                self.help.register(
                    response.rect,
                    if editing_target {
                        help::HelpTopic::Target
                    } else {
                        help::HelpTopic::Path
                    },
                );
            }
            if response.clicked() {
                clicked_key = Some((selection, *index, *time));
            }
        }
        if let Some((selection, _index, time)) = clicked_key {
            self.anim_editor.selection = Some(selection);
            self.anim_edit_target = editing_target;
            self.rig_time = time;
        }
        if self.project.rig.target.mode == TargetMode::Fixed && !editing_target {
            let selected =
                self.anim_editor.selection == Some(animation_ui::AnimSelection::FixedTarget);
            let fixed_target = ui.selectable_label(selected, "Cible fixe");
            if fixed_target.hovered() {
                self.help
                    .register(fixed_target.rect, help::HelpTopic::Target);
            }
            if fixed_target.clicked() {
                self.anim_editor.selection = Some(animation_ui::AnimSelection::FixedTarget);
            }
        }

        let mut changed = false;
        let fps_keys = self.project.rig.fps.max(1) as f32;
        match self.anim_editor.selection {
            Some(animation_ui::AnimSelection::EyeKey(index))
                if !editing_target && index < self.project.rig.eye.keys.len() =>
            {
                if let Some(key) = self.project.rig.eye.keys.get_mut(index) {
                    ui.horizontal(|ui| {
                        ui.label("Temps");
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut key.time_s)
                                    .speed(0.02)
                                    .range(0.0..=600.0)
                                    .suffix(" s"),
                            )
                            .changed();
                        let mut frame = (key.time_s * fps_keys).round() as i64;
                        if ui
                            .add(
                                egui::DragValue::new(&mut frame)
                                    .speed(1.0)
                                    .range(0..=(600.0 * fps_keys) as i64)
                                    .prefix("image "),
                            )
                            .on_hover_text(
                                "Temps exprimé en images ; modifier synchronise les secondes.",
                            )
                            .changed()
                        {
                            key.time_s = (frame as f32 / fps_keys).clamp(0.0, 600.0);
                            changed = true;
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        for axis in 0..3 {
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut key.position[axis])
                                        .speed(10.0)
                                        .prefix(["X ", "Y ", "Z "][axis]),
                                )
                                .changed();
                        }
                    });
                    changed |= param_f32(ui, "Adoucissement", "", &mut key.ease, 0.0, 1.0, 0.0);
                    if ui.button("Poignées automatiques").clicked() {
                        key.in_handle = [0.0; 3];
                        key.out_handle = [0.0; 3];
                        changed = true;
                    }
                }
                if changed {
                    self.project
                        .rig
                        .eye
                        .keys
                        .sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
                    self.mark_animation_changed();
                }
            }
            Some(animation_ui::AnimSelection::TargetKey(index))
                if editing_target && index < self.project.rig.target.path.keys.len() =>
            {
                if let Some(key) = self.project.rig.target.path.keys.get_mut(index) {
                    ui.horizontal(|ui| {
                        ui.label("Temps");
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut key.time_s)
                                    .speed(0.02)
                                    .range(0.0..=600.0)
                                    .suffix(" s"),
                            )
                            .changed();
                        let mut frame = (key.time_s * fps_keys).round() as i64;
                        if ui
                            .add(
                                egui::DragValue::new(&mut frame)
                                    .speed(1.0)
                                    .range(0..=(600.0 * fps_keys) as i64)
                                    .prefix("image "),
                            )
                            .changed()
                        {
                            key.time_s = (frame as f32 / fps_keys).clamp(0.0, 600.0);
                            changed = true;
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        for axis in 0..3 {
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut key.position[axis])
                                        .speed(10.0)
                                        .prefix(["X ", "Y ", "Z "][axis]),
                                )
                                .changed();
                        }
                    });
                    changed |= param_f32(ui, "Adoucissement", "", &mut key.ease, 0.0, 1.0, 0.0);
                }
                if changed {
                    self.project
                        .rig
                        .target
                        .path
                        .keys
                        .sort_by(|a, b| a.time_s.total_cmp(&b.time_s));
                    self.mark_animation_changed();
                }
            }
            Some(animation_ui::AnimSelection::FixedTarget)
                if !editing_target && self.project.rig.target.mode == TargetMode::Fixed =>
            {
                ui.horizontal_wrapped(|ui| {
                    for axis in 0..3 {
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut self.project.rig.target.fixed[axis])
                                    .speed(10.0)
                                    .prefix(["X ", "Y ", "Z "][axis]),
                            )
                            .changed();
                    }
                });
                if changed {
                    self.mark_animation_changed();
                }
            }
            _ => {
                ui.label(
                    egui::RichText::new("Sélectionnez une clé dans la liste ou dans le viewport.")
                        .small()
                        .weak(),
                );
            }
        }

        ui.horizontal_wrapped(|ui| {
            let add = ui.button("+ Clé au temps courant");
            if add.hovered() {
                self.help.register(add.rect, help::HelpTopic::Path);
            }
            if add.clicked() {
                self.add_animation_key(editing_target);
            }
            let can_delete = match self.anim_editor.selection {
                Some(animation_ui::AnimSelection::EyeKey(index)) => {
                    self.project.rig.eye.keys.len() > 2 && index < self.project.rig.eye.keys.len()
                }
                Some(animation_ui::AnimSelection::TargetKey(index)) => {
                    self.project.rig.target.path.keys.len() > 2
                        && index < self.project.rig.target.path.keys.len()
                }
                _ => false,
            };
            if ui
                .add_enabled(can_delete, egui::Button::new("Supprimer la clé"))
                .clicked()
            {
                self.delete_animation_key();
            }
            if ui
                .button("Insérer au temps courant")
                .on_hover_text("Ajoute une clé sur le segment, sans déplacer les voisines")
                .clicked()
            {
                self.insert_animation_key(editing_target);
            }
        });
        ui.label(
            egui::RichText::new(
                "Astuce : en mode Tracer, chaque clic sur le terrain ajoute une clé et allonge la durée.",
            )
            .small()
            .weak(),
        );
    }

    fn ensure_target_path(&mut self) {
        if self.project.rig.target.path.keys.len() >= 2 {
            return;
        }
        let duration = self.project.rig.duration_s.max(1.0);
        let center = self.terrain_center_point();
        self.project.rig.target.mode = TargetMode::Path;
        self.project.rig.target.path.keys =
            vec![PathKey::new(0.0, center), PathKey::new(duration, center)];
    }

    /// Orbit the design camera around a short animation loop so its
    /// trajectory is readable without losing the terrain context.
    fn frame_animation_loop(&mut self) {
        let center = vf_flight::rig::path_center(&self.project.rig.eye);
        let length = vf_flight::rig::path_length(&self.project.rig.eye);
        let distance = (length * 1.15).clamp(420.0, 2600.0);
        self.orbit_center = center;
        self.project.camera = vf_scene::orbit(center, distance, 35.0, 30.0, self.fov);
        self.sync_orbit_from_camera();
        self.mark_camera_dirty();
    }

    fn terrain_center_point(&self) -> [f64; 3] {
        let bounds = self.heightfield.bounds();
        [
            (bounds.min[0] + bounds.max[0]) * 0.5,
            (bounds.min[1] + bounds.max[1]) * 0.5,
            bounds.min[2] + (bounds.max[2] - bounds.min[2]) * 0.38,
        ]
    }

    fn add_animation_key(&mut self, target: bool) {
        if target {
            self.ensure_target_path();
        }
        let time = self
            .rig_time
            .clamp(0.0, self.project.rig.duration_s.max(0.1));
        let fallback = self.terrain_center_point();
        let position = if target {
            let p = vf_flight::rig::sample_path(&self.project.rig.target.path, time);
            [p.x, p.y, p.z]
        } else {
            let p = vf_flight::rig::sample_path(&self.project.rig.eye, time);
            [p.x, p.y, p.z + 80.0]
        };
        let position = if position.iter().all(|value| value.is_finite()) {
            position
        } else {
            fallback
        };
        let path = if target {
            &mut self.project.rig.target.path
        } else {
            &mut self.project.rig.eye
        };
        path.push(PathKey::new(time, position));
        let index = path
            .keys
            .iter()
            .position(|key| (key.time_s - time).abs() < 1e-4);
        self.anim_editor.selection = match (target, index) {
            (true, Some(index)) => Some(animation_ui::AnimSelection::TargetKey(index)),
            (false, Some(index)) => Some(animation_ui::AnimSelection::EyeKey(index)),
            _ => None,
        };
        self.anim_edit_target = target;
        self.mark_animation_changed();
    }

    fn insert_animation_key(&mut self, target: bool) {
        let time = self
            .rig_time
            .clamp(0.0, self.project.rig.duration_s.max(0.1));
        self.insert_animation_key_at(target, time);
    }

    fn insert_animation_key_at(&mut self, target: bool, time: f32) {
        let time = time.clamp(0.0, 600.0);
        let path = if target {
            &self.project.rig.target.path
        } else {
            &self.project.rig.eye
        };
        if path.keys.len() < 2 {
            self.rig_status = "Le tracé a besoin d'au moins deux clés".into();
            return;
        }
        if let Some(index) = path
            .keys
            .iter()
            .position(|key| (key.time_s - time).abs() < 0.02)
        {
            self.anim_editor.selection = Some(if target {
                animation_ui::AnimSelection::TargetKey(index)
            } else {
                animation_ui::AnimSelection::EyeKey(index)
            });
            self.anim_edit_target = target;
            self.rig_status = format!("Une clé existe déjà à t={time:.2}s");
            return;
        }
        let position = vf_flight::rig::sample_path(path, time);
        let new_key = PathKey::new(time, [position.x, position.y, position.z]);
        let path = if target {
            &mut self.project.rig.target.path
        } else {
            &mut self.project.rig.eye
        };
        path.push(new_key);
        let index = path
            .keys
            .iter()
            .position(|key| (key.time_s - time).abs() < 1e-4);
        self.anim_editor.selection = match (target, index) {
            (true, Some(index)) => Some(animation_ui::AnimSelection::TargetKey(index)),
            (false, Some(index)) => Some(animation_ui::AnimSelection::EyeKey(index)),
            _ => None,
        };
        self.anim_edit_target = target;
        self.mark_animation_changed();
        self.rig_status = format!("Clé insérée à t={time:.2}s");
    }

    fn delete_animation_key(&mut self) {
        if let Some(selection) = self.anim_editor.selection {
            let removed = match selection {
                animation_ui::AnimSelection::EyeKey(index)
                    if self.project.rig.eye.keys.len() > 2 =>
                {
                    self.project.rig.eye.keys.remove(index);
                    true
                }
                animation_ui::AnimSelection::TargetKey(index)
                    if self.project.rig.target.path.keys.len() > 2 =>
                {
                    self.project.rig.target.path.keys.remove(index);
                    true
                }
                _ => false,
            };
            if removed {
                self.anim_editor.selection = None;
                self.mark_animation_changed();
            }
        }
    }

    /// Build the speed/bank curves used by the graph editor. They come from
    /// the same precomputed track as playback, so the graph reflects the
    /// selected motion profile rather than an idealised curve.
    fn ensure_timeline_curves(&mut self) {
        let stale = self
            .anim_curve_cache
            .as_ref()
            .is_none_or(|(revision, _)| *revision != self.anim_revision);
        if !stale {
            return;
        }
        let Some(track) = self.animation_track() else {
            return;
        };
        let samples = 160;
        let mut curves = timeline::Curves::default();
        for index in 0..=samples {
            let t = track.duration_s() * index as f32 / samples as f32;
            let sample = track.sample_at(t);
            curves.speed.push((t, sample.speed_mps));
            curves.bank.push((t, sample.roll_deg));
        }
        self.anim_curve_cache = Some((self.anim_revision, curves));
    }

    fn animation_timeline_ui(&mut self, ui: &mut egui::Ui) {
        let needs_curves = self.anim_timeline.mode == timeline::Mode::Graph
            && matches!(
                self.anim_timeline.channel,
                timeline::Channel::Speed | timeline::Channel::Bank
            );
        if needs_curves && !self.anim_timeline.is_dragging() {
            self.ensure_timeline_curves();
        }
        let blocked = self.help.last_rect().union(self.anim_transport_rect);
        let duration = self.project.rig.duration_s;
        let curves = if needs_curves {
            self.anim_curve_cache.as_ref().map(|(_, curves)| curves)
        } else {
            None
        };
        let output = timeline::show(
            ui,
            &mut self.anim_timeline,
            timeline::TimelineInput {
                rig: &mut self.project.rig,
                selection: &mut self.anim_editor.selection,
                time: &mut self.rig_time,
                edit_target: &mut self.anim_edit_target,
                duration,
                curves,
                blocked,
            },
        );
        if let Some(seek) = output.seek {
            self.rig_time = seek.clamp(0.0, self.project.rig.duration_s.max(0.1));
            self.anim_playing = false;
            self.anim_live_sample = None;
            if self.anim_view == animation_ui::AnimViewMode::Camera {
                self.anim_camera_dirty = true;
            }
            ui.ctx().request_repaint();
        }
        if let Some((target, time)) = output.insert {
            self.rig_time = time.clamp(0.0, 600.0);
            self.insert_animation_key_at(target, self.rig_time);
        }
        if output.delete {
            self.delete_animation_key();
        }
        if output.changed {
            self.mark_animation_changed();
            ui.ctx().request_repaint();
        }
        if let Some(status) = output.status {
            self.rig_status = status;
        }
    }

    /// Replace the target path with a copy of the eye path, optionally
    /// offset in world space. This is the quickest way to create a linked
    /// "camera + target" pair while keeping both editable.
    fn copy_eye_path_to_target(&mut self, offset: [f64; 3]) {
        self.project.rig.target.mode = TargetMode::Path;
        // Keep the offset in `TargetRig::offset` instead of baking it into the
        // keys: the renderer already adds that offset, and baking it would
        // apply it twice. The target path remains an exact copy of the eye
        // path and stays editable on its own.
        self.project.rig.target.path.keys = self.project.rig.eye.keys.clone();
        self.project.rig.target.offset = offset;
        self.anim_edit_target = true;
        self.anim_editor.selection = None;
        self.mark_animation_changed();
        self.rig_status = "Cible : copie du tracé caméra".into();
    }

    /// Copy the eye key times onto the target path when both have the same
    /// number of keys. This keeps a target path linked to the camera timing.
    fn sync_target_times(&mut self) -> bool {
        if self.project.rig.target.mode != TargetMode::Path
            || self.project.rig.target.path.keys.len() != self.project.rig.eye.keys.len()
        {
            return false;
        }
        let mut changed = false;
        for index in 0..self.project.rig.eye.keys.len() {
            let time = self.project.rig.eye.keys[index].time_s;
            let target = &mut self.project.rig.target.path.keys[index];
            if (target.time_s - time).abs() > 1e-5 {
                target.time_s = time;
                changed = true;
            }
        }
        changed
    }

    fn retime_animation_by_distance(&mut self) {
        let duration = self.project.rig.duration_s;
        let mut rig = self.project.rig.clone();
        if vf_flight::rig::retime_path_by_distance(&mut rig.eye, duration) {
            self.project.rig.eye = rig.eye;
            self.project.rig.duration_s = duration;
            let synced = self.sync_target_times();
            self.mark_animation_changed();
            self.rig_status = format!(
                "Temps répartis par distance sur {duration:.2}s{}",
                if synced {
                    " · cible synchronisée"
                } else {
                    ""
                }
            );
        }
    }

    fn retime_animation_to_speed(&mut self, speed_mps: f32) {
        let mut rig = self.project.rig.clone();
        if let Some(duration) = vf_flight::rig::retime_path_to_speed(&mut rig.eye, speed_mps) {
            self.project.rig.eye = rig.eye;
            self.project.rig.duration_s = duration;
            let synced = self.sync_target_times();
            self.rig_time = self.rig_time.min(duration);
            self.mark_animation_changed();
            self.rig_status = format!(
                "Vitesse cible {speed_mps:.1} m/s · durée {duration:.2}s{}",
                if synced {
                    " · cible synchronisée"
                } else {
                    ""
                }
            );
        } else {
            self.rig_status = "Vitesse cible invalide".into();
        }
    }

    /// Change the clip duration. When `stretch` is enabled (the default), the
    /// authored key times are scaled to fill the new clip, so the camera keeps
    /// following the whole curve instead of reaching the last key early and
    /// waiting there.
    fn set_animation_duration(&mut self, duration: f32) {
        let old = self.project.rig.duration_s;
        let changed = if self.anim_stretch_keys {
            self.project.rig.stretch_duration(duration)
        } else {
            let new = duration.clamp(0.1, 600.0);
            let changed = (new - old).abs() > 1e-6;
            self.project.rig.duration_s = new;
            changed
        };
        if changed {
            self.rig_time = self.rig_time.min(self.project.rig.duration_s);
            self.anim_live_sample = None;
            self.mark_animation_changed();
            self.rig_status = format!(
                "Durée {:.2}s{}",
                self.project.rig.duration_s,
                if self.anim_stretch_keys {
                    " · clés étirées en temps"
                } else {
                    ""
                }
            );
        }
    }

    fn anim_timing_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(
            egui::RichText::new(
                "Étape 2 · Réglez la durée, la cadence et l'optique. La durée peut rester indépendante du dernier point.",
            )
            .small(),
        );
        let mut changed = false;
        changed |= duration_param(ui, &mut self.project.rig, self.anim_stretch_keys);
        ui.checkbox(
            &mut self.anim_stretch_keys,
            "Étirer les clés avec la durée",
        )
        .on_hover_text(
            "La durée étire le temps des clés (les positions 3D ne changent pas). Désactivé, les clés gardent leur temps absolu et la caméra peut atteindre la fin avant la durée.",
        );
        if ui
            .checkbox(
                &mut self.project.rig.exact_path,
                "Suivre exactement le tracé (sans inertie)",
            )
            .on_hover_text(
                "La caméra est exactement sur la courbe aux temps des clés : la vitesse correspond aux distances et aux images. Décochez pour laisser le profil physique (ressort, traînée, évitement) déformer la trajectoire.",
            )
            .changed()
        {
            self.mark_animation_changed();
            self.rig_status = if self.project.rig.exact_path {
                "Suivi exact du tracé activé : vitesse = distance / temps".into()
            } else {
                "Inertie du profil activée : la caméra peut couper les virages".into()
            };
        }
        let key_end = self.project.rig.eye.duration_s();
        let duration = self.project.rig.duration_s;
        if (key_end - duration).abs() > 0.05 {
            ui.horizontal_wrapped(|ui| {
                let text = if key_end > duration {
                    format!(
                        "Des clés dépassent la fin ({key_end:.2} s > {duration:.2} s) : elles restent conservées mais ne sont pas jouées."
                    )
                } else {
                    format!(
                        "Le tracé se termine à {key_end:.2} s, la durée est {duration:.2} s : la caméra atteindra la fin puis attendra. Étirez les clés pour remplir le clip."
                    )
                };
                ui.label(
                    egui::RichText::new(text)
                        .size(10.0)
                        .color(egui::Color32::from_rgb(255, 196, 110)),
                );
                if key_end > duration {
                    if ui.button("Étendre à la dernière clé").clicked() {
                        self.set_animation_duration(key_end);
                        changed = true;
                    }
                } else {
                    if ui
                        .button("Étirer les clés sur la durée")
                        .on_hover_text(
                            "Rééchelonne les temps des clés pour que le tracé occupe toute la durée, sans changer les positions 3D.",
                        )
                        .clicked()
                    {
                        let duration = self.project.rig.duration_s;
                        if self.project.rig.stretch_duration(duration) {
                            self.mark_animation_changed();
                            changed = true;
                        }
                    }
                    if ui.button("Régler la durée sur le tracé").clicked() {
                        self.set_animation_duration(key_end);
                        changed = true;
                    }
                }
            });
        }
        changed |= param_u32(ui, "Images/s", "", &mut self.project.rig.fps, 1, 120, 24);
        let frames = self.project.rig.frame_count();
        ui.label(
            egui::RichText::new(format!(
                "{frames} images · {:.2} s · {} fps",
                self.project.rig.duration_s, self.project.rig.fps
            ))
            .small()
            .weak(),
        );
        let fit = animation_ui::TimingFit::measure(&self.project.rig);
        let (fit_color, fit_icon) = if fit.is_strained() {
            (egui::Color32::from_rgb(255, 176, 100), "Attention :")
        } else if fit.pressure < 0.45 {
            (egui::Color32::from_rgb(150, 190, 230), "Info :")
        } else {
            (egui::Color32::from_rgb(130, 210, 160), "OK :")
        };
        egui::Frame::group(ui.style())
            .fill(egui::Color32::from_rgba_unmultiplied(22, 28, 38, 180))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(format!(
                        "{fit_icon}  {:.0} m de tracé · {:.0} m/s demandés · croisière {:.0} m/s",
                        fit.length_m, fit.required_speed_mps, fit.cruise_speed_mps
                    ))
                    .size(10.5)
                    .color(fit_color),
                );
                ui.label(
                    egui::RichText::new(fit.advice())
                        .size(10.0)
                        .color(egui::Color32::from_rgb(180, 195, 212)),
                );
                if (fit.is_strained() || fit.pressure < 0.45)
                    && ui
                        .button(format!(
                            "Ajuster la durée à {:.0} s (conseillé)",
                            fit.recommended_duration_s
                        ))
                        .clicked()
                {
                    self.set_animation_duration(fit.recommended_duration_s);
                }
            });
        ui.horizontal(|ui| {
            ui.label("Seed");
            changed |= ui
                .add(egui::DragValue::new(&mut self.project.rig.seed).speed(1.0))
                .changed();
        });
        if changed {
            self.rig_time = self.rig_time.min(self.project.rig.duration_s);
            self.mark_animation_changed();
        }

        ui.separator();
        ui.label(
            egui::RichText::new("Timing par segment")
                .strong()
                .size(13.0),
        )
        .on_hover_text(
            "Distance réelle de la courbe, durée entre les clés, vitesse moyenne et nombre d'images.",
        );
        let segments = vf_flight::rig::path_segments(&self.project.rig.eye);
        if segments.is_empty() {
            ui.label(
                egui::RichText::new("Ajoutez au moins deux clés.")
                    .small()
                    .weak(),
            );
        } else {
            let max_speed = self.project.rig.body.max_speed_mps;
            let fps = self.project.rig.fps.max(1) as f32;
            egui::ScrollArea::vertical()
                .max_height(150.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Grid::new("anim-segment-grid")
                        .num_columns(5)
                        .striped(true)
                        .show(ui, |ui| {
                            for header in ["Segment", "Longueur", "Durée", "Vitesse", "Images"] {
                                ui.label(egui::RichText::new(header).small().strong());
                            }
                            ui.end_row();
                            for segment in &segments {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "E{}→E{}",
                                        segment.index,
                                        segment.index + 1
                                    ))
                                    .monospace()
                                    .size(10.0),
                                );
                                ui.label(format!("{:.0} m", segment.length_m));
                                ui.label(format!("{:.2} s", segment.duration_s()));
                                let speed = segment.average_speed_mps;
                                ui.label(egui::RichText::new(format!("{speed:.1} m/s")).color(
                                    if speed > max_speed + 0.5 {
                                        egui::Color32::from_rgb(255, 150, 110)
                                    } else {
                                        egui::Color32::from_rgb(170, 210, 180)
                                    },
                                ));
                                ui.label(format!("{:.0}", (segment.duration_s() * fps).round()));
                                ui.end_row();
                            }
                        });
                });
            let total: f64 = segments.iter().map(|segment| segment.length_m).sum();
            ui.label(
                egui::RichText::new(format!(
                    "Total {:.0} m · {:.2}s · {} images",
                    total,
                    self.project.rig.duration_s,
                    self.project.rig.frame_count()
                ))
                .small()
                .weak(),
            );
        }
        if self.project.rig.target.mode == TargetMode::Path {
            let target_segments = vf_flight::rig::path_segments(&self.project.rig.target.path);
            if !target_segments.is_empty() {
                ui.label(
                    egui::RichText::new("Timing de la cible")
                        .strong()
                        .size(12.0),
                );
                let max_speed = self.project.rig.body.max_speed_mps;
                let fps = self.project.rig.fps.max(1) as f32;
                egui::ScrollArea::vertical()
                    .max_height(110.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui::Grid::new("anim-target-segment-grid")
                            .num_columns(5)
                            .striped(true)
                            .show(ui, |ui| {
                                for header in ["Segment", "Longueur", "Durée", "Vitesse", "Images"]
                                {
                                    ui.label(egui::RichText::new(header).small().strong());
                                }
                                ui.end_row();
                                for segment in &target_segments {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "T{}→T{}",
                                            segment.index,
                                            segment.index + 1
                                        ))
                                        .monospace()
                                        .size(10.0),
                                    );
                                    ui.label(format!("{:.0} m", segment.length_m));
                                    ui.label(format!("{:.2} s", segment.duration_s()));
                                    let speed = segment.average_speed_mps;
                                    ui.label(egui::RichText::new(format!("{speed:.1} m/s")).color(
                                        if speed > max_speed + 0.5 {
                                            egui::Color32::from_rgb(255, 150, 110)
                                        } else {
                                            egui::Color32::from_rgb(170, 210, 180)
                                        },
                                    ));
                                    ui.label(format!(
                                        "{:.0}",
                                        (segment.duration_s() * fps).round()
                                    ));
                                    ui.end_row();
                                }
                            });
                    });
            }
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("Répartir les temps par distance")
                .on_hover_text(
                    "Rend la vitesse moyenne identique sur chaque segment, à durée totale constante.",
                )
                .clicked()
            {
                self.retime_animation_by_distance();
            }
            ui.label("Vitesse cible");
            ui.add(
                egui::DragValue::new(&mut self.anim_target_speed)
                    .speed(0.5)
                    .range(0.0..=200.0)
                    .suffix(" m/s"),
            )
            .on_hover_text("0 = vitesse de croisière du profil.");
            if ui.button("Appliquer la vitesse").clicked() {
                let speed = if self.anim_target_speed > 0.1 {
                    self.anim_target_speed
                } else {
                    vf_flight::rig::cruise_speed_mps(&self.project.rig.body)
                };
                self.retime_animation_to_speed(speed);
            }
            if ui
                .button("Ease 0")
                .on_hover_text("Vitesse constante à l'intérieur de chaque segment.")
                .clicked()
            {
                self.flatten_key_easing();
            }
            if self.project.rig.target.mode == TargetMode::Path
                && self.project.rig.target.path.keys.len() == self.project.rig.eye.keys.len()
                && ui
                    .button("Synchroniser les temps cible")
                    .on_hover_text("Copie les temps des clés caméra sur la cible.")
                    .clicked()
                && self.sync_target_times()
            {
                self.mark_animation_changed();
                self.rig_status = "Temps de la cible synchronisés sur la caméra".into();
            }
        });
        ui.label(
            egui::RichText::new(
                "Une vitesse rouge dépasse la vitesse max du profil : allongez la durée ou choisissez un autre profil.",
            )
            .small()
            .weak(),
        );

        let header = egui::CollapsingHeader::new("Optique et caméra")
            .default_open(true)
            .show(ui, |ui| {
                let mut optics_changed = param_f32(
                    ui,
                    "FOV",
                    "°",
                    &mut self.project.rig.optics.fov_y_deg,
                    5.0,
                    150.0,
                    50.0,
                );
                ui.checkbox(
                    &mut self.project.rig.optics.dof_enabled,
                    "Profondeur de champ",
                );
                if self.project.rig.optics.dof_enabled {
                    optics_changed |= param_f32(
                        ui,
                        "Ouverture",
                        "f/",
                        &mut self.project.rig.optics.fstop,
                        0.7,
                        64.0,
                        8.0,
                    );
                    ui.checkbox(
                        &mut self.project.rig.optics.focus_on_target,
                        "Mise au point sur la cible",
                    );
                    if !self.project.rig.optics.focus_on_target {
                        optics_changed |= param_f32(
                            ui,
                            "Distance",
                            "m",
                            &mut self.project.rig.optics.focus_distance_m,
                            0.1,
                            100_000.0,
                            100.0,
                        );
                    }
                }
                optics_changed |= param_f32(
                    ui,
                    "Roulis",
                    "°",
                    &mut self.project.rig.optics.roll_deg,
                    -180.0,
                    180.0,
                    0.0,
                );
                ui.horizontal(|ui| {
                    ui.label("Liaison caméra");
                    let mut link = self.project.rig.optics.link;
                    egui::ComboBox::from_id_salt("anim-camera-link")
                        .selected_text(match link {
                            CameraLink::Eye => "Rigide",
                            CameraLink::Spring => "Ressort",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut link, CameraLink::Eye, "Rigide");
                            ui.selectable_value(&mut link, CameraLink::Spring, "Ressort");
                        });
                    if link != self.project.rig.optics.link {
                        self.project.rig.optics.link = link;
                        optics_changed = true;
                    }
                });
                if self.project.rig.optics.link == CameraLink::Spring {
                    optics_changed |= param_f32(
                        ui,
                        "Raideur",
                        "",
                        &mut self.project.rig.optics.spring_stiffness,
                        0.1,
                        40.0,
                        6.0,
                    );
                    optics_changed |= param_f32(
                        ui,
                        "Amortissement",
                        "",
                        &mut self.project.rig.optics.spring_damping,
                        0.0,
                        3.0,
                        0.8,
                    );
                }
                if optics_changed {
                    self.mark_animation_changed();
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Optics,
        );

        let header = egui::CollapsingHeader::new("Altitude relative au sol (AGL)")
            .default_open(false)
            .show(ui, |ui| {
                let constraints = &mut self.project.rig.constraints;
                let mut changed = false;
                changed |= ui
                    .checkbox(
                        &mut constraints.relative_altitude,
                        "Interpréter la hauteur des clés comme une altitude au-dessus du sol",
                    )
                    .on_hover_text(
                        "La trajectoire horizontale et son timing ne changent pas : le Z authoré devient une hauteur AGL. La caméra et la cible suivent le relief avec leur propre amortissement, au lieu de traverser les collines.",
                    )
                    .changed();
                if constraints.relative_altitude {
                    changed |= param_f32(
                        ui,
                        "Amortissement caméra",
                        "s",
                        &mut constraints.camera_altitude_damping_s,
                        0.0,
                        10.0,
                        0.9,
                    );
                    changed |= param_f32(
                        ui,
                        "Amortissement cible",
                        "s",
                        &mut constraints.target_altitude_damping_s,
                        0.0,
                        10.0,
                        1.6,
                    );
                    changed |= param_f32(
                        ui,
                        "Anticipation relief",
                        "s",
                        &mut constraints.altitude_look_ahead_s,
                        0.0,
                        5.0,
                        0.8,
                    );
                    changed |= param_f32(
                        ui,
                        "Empreinte au sol",
                        "m",
                        &mut constraints.altitude_footprint_m,
                        0.0,
                        2000.0,
                        60.0,
                    );
                    ui.label(
                        egui::RichText::new(
                            "0 s = suivi brut du relief (nerveux) ; 1–3 s = survol stabilisé. La hauteur AGL est bornée par « Hauteur sol » ci-dessous.",
                        )
                        .small()
                        .weak(),
                    );
                    let camera_agm = constraints.ground_clearance_m;
                    changed |= param_f32(
                        ui,
                        "Plancher AGL",
                        "m",
                        &mut constraints.ground_clearance_m,
                        0.0,
                        2000.0,
                        camera_agm,
                    );
                }
                if changed {
                    self.mark_animation_changed();
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Constraints,
        );

        let header = egui::CollapsingHeader::new("Contraintes")
            .default_open(false)
            .show(ui, |ui| {
                let constraints = &mut self.project.rig.constraints;
                let mut changed = false;
                changed |= ui
                    .checkbox(&mut constraints.enabled, "Contraintes actives")
                    .changed();
                ui.separator();
                changed |= ui
                    .checkbox(&mut constraints.ground_guard, "Garde au sol amortie")
                    .on_hover_text(
                        "Maintient le trajet caméra et la caméra optique au-dessus du terrain, même en suivi exact. La correction est lissée et un plancher dur évite toute traversée.",
                    )
                    .changed();
                changed |= param_f32(
                    ui,
                    "Hauteur sol",
                    "m",
                    &mut constraints.ground_clearance_m,
                    0.0,
                    2000.0,
                    25.0,
                );
                changed |= ui
                    .checkbox(&mut constraints.reference_enabled, "Sol de référence fixe")
                    .changed();
                if constraints.reference_enabled {
                    changed |= param_f32(
                        ui,
                        "Référence",
                        "m",
                        &mut constraints.reference_z_m,
                        -10_000.0,
                        20_000.0,
                        0.0,
                    );
                }
                ui.separator();
                changed |= ui
                    .checkbox(&mut constraints.wall_avoidance, "Évitement des parois")
                    .on_hover_text(
                        "Détecte les parois devant la caméra, monte si l'accélération verticale le permet, sinon contourne latéralement puis revient sur le tracé.",
                    )
                    .changed();
                changed |= param_f32(
                    ui,
                    "Distance parois",
                    "m",
                    &mut constraints.wall_distance_m,
                    0.0,
                    2000.0,
                    40.0,
                );
                changed |= param_f32(
                    ui,
                    "Marge caméra",
                    "m",
                    &mut constraints.wall_margin_m,
                    0.0,
                    200.0,
                    5.0,
                );
                changed |= param_f32(
                    ui,
                    "Angle d'impact",
                    "°",
                    &mut constraints.wall_angle_deg,
                    0.0,
                    90.0,
                    20.0,
                );
                changed |= param_f32(
                    ui,
                    "Anticipation paroi",
                    "s",
                    &mut constraints.wall_look_ahead_s,
                    0.2,
                    10.0,
                    3.0,
                );
                changed |= param_f32(
                    ui,
                    "Détour max",
                    "m",
                    &mut constraints.wall_detour_m,
                    0.0,
                    2000.0,
                    0.0,
                );
                changed |= param_f32(
                    ui,
                    "Marge verticale",
                    "m",
                    &mut constraints.wall_vertical_margin_m,
                    0.0,
                    500.0,
                    5.0,
                );
                ui.label(
                    egui::RichText::new("Détour max 0 = automatique (distance + marge + 20 m).")
                        .small()
                        .weak(),
                );
                changed |= ui
                    .checkbox(&mut constraints.apply_eye, "Appliquer au trajet caméra")
                    .changed();
                changed |= ui
                    .checkbox(&mut constraints.apply_camera, "Appliquer à la caméra optique")
                    .changed();
                changed |= ui
                    .checkbox(&mut constraints.apply_target, "Appliquer à la cible")
                    .changed();
                changed |= param_f32(ui, "Douceur", "", &mut constraints.softness, 0.0, 1.0, 0.35);
                if changed {
                    self.mark_animation_changed();
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Constraints,
        );

        ui.horizontal(|ui| {
            if ui.button("Vue caméra à t").clicked() {
                self.enter_animation_camera(ctx);
            }
            if ui.button("Vue design").clicked() {
                self.enter_animation_design(ctx);
            }
        });
    }

    fn anim_profile_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(
            egui::RichText::new(
                "Étape 3 · Choisissez un profil : il change la dynamique du corps, pas votre tracé. Les courbes comparent les profils sur ce trajet.",
            )
            .small(),
        );
        if self.project.rig.exact_path {
            ui.label(
                egui::RichText::new(
                    "Suivi exact actif : le profil ne modifie pas la vitesse de position, seulement le roulis et la visée. Décochez le suivi exact pour retrouver l'inertie (les courbes de comparaison utilisent déjà l'inertie).",
                )
                .size(10.0)
                .color(egui::Color32::from_rgb(255, 196, 110)),
            );
        }
        if self
            .anim_profile_cache
            .as_ref()
            .is_none_or(|cache| cache.revision != self.anim_revision)
        {
            let curves = animation_ui::profile_curves(
                &self.project.rig,
                &self.heightfield,
                self.project.seed,
            );
            self.anim_profile_cache = Some(animation_ui::ProfileCache {
                revision: self.anim_revision,
                curves,
            });
        }
        let active = self.project.rig.body.preset;
        ui.horizontal_wrapped(|ui| {
            for preset in AircraftPreset::ALL {
                let color = animation_ui::preset_color(preset);
                let label = if preset == active {
                    format!("✓ {}", preset.label())
                } else {
                    preset.label().to_string()
                };
                let button = egui::Button::new(egui::RichText::new(label).size(12.0))
                    .fill(color.gamma_multiply(0.25));
                let response = ui.add(button);
                if response.hovered() {
                    self.help.register(response.rect, help::HelpTopic::Profile);
                }
                if response.clicked() && preset != active {
                    animation_ui::apply_aircraft_preset(&mut self.project.rig, preset);
                    self.mark_animation_changed();
                    self.rig_status = format!(
                        "Profil {} : {}",
                        preset.label(),
                        animation_ui::preset_character(preset)
                    );
                }
            }
        });
        ui.label(
            egui::RichText::new(animation_ui::preset_character(active))
                .small()
                .weak(),
        );
        let fit = animation_ui::TimingFit::measure(&self.project.rig);
        if fit.is_strained() {
            ui.label(
                egui::RichText::new(format!(
                    "Attention : ce tracé demande {:.0} m/s alors que le profil plafonne à {:.0} m/s : les profils vont se ressembler. Raccourcissez le tracé ou passez à ≈ {:.0} s.",
                    fit.required_speed_mps,
                    fit.max_speed_mps,
                    fit.recommended_duration_s
                ))
                .size(10.0)
                .color(egui::Color32::from_rgb(255, 176, 100)),
            );
            ui.horizontal_wrapped(|ui| {
                if ui
                    .button(format!("Ajuster à {:.0} s", fit.recommended_duration_s))
                    .clicked()
                {
                    self.set_animation_duration(fit.recommended_duration_s);
                }
            });
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("Tracé de démonstration (S)")
                .on_hover_text(
                    "Crée un S adapté au profil, à 90 % de sa vitesse max : idéal pour comparer les profils en quelques secondes",
                )
                .clicked()
            {
                match vf_flight::rig::preset_path(
                    &mut self.project.rig,
                    &self.heightfield,
                    "demo",
                ) {
                    Ok(()) => {
                        self.anim_editor.selection = None;
                        self.anim_edit_target = false;
                        self.rig_time = 0.0;
                        self.mark_animation_changed();
                        self.rig_status = format!(
                            "Tracé de démonstration : {:.1}s, {}",
                            self.project.rig.duration_s,
                            active.label()
                        );
                        // Frame the small loop so the five trajectories are
                        // actually visible in the design viewport.
                        self.frame_animation_loop();
                    }
                    Err(error) => self.rig_status = error,
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .add(egui::Button::new(egui::RichText::new("Tester ce profil").strong()))
                .on_hover_text("Lance la lecture dans la vue courante (design ou caméra)")
                .clicked()
            {
                self.rig_time = 0.0;
                self.toggle_animation_play(ctx);
            }
            let mut compare = self.anim_compare;
            if ui
                .checkbox(&mut compare, "Comparer les trajectoires")
                .on_hover_text(
                    "Affiche les trajectoires simulées des cinq corps sur le même tracé, en vue design",
                )
                .changed()
            {
                self.anim_compare = compare;
                self.anim_compare_cache = None;
                self.anim_view = animation_ui::AnimViewMode::Design;
                self.anim_playing = false;
            }
            if self.anim_compare {
                ui.checkbox(&mut self.anim_editor.show_trail, "Jaune = profil actif");
            }
        });
        ui.add_space(4.0);
        let curves = self
            .anim_profile_cache
            .as_ref()
            .map_or(&[][..], |cache| cache.curves.as_slice());
        let chart = animation_ui::profile_chart(ui, curves, active);
        if chart.hovered() {
            self.help.register(chart.rect, help::HelpTopic::Profile);
        }
        ui.horizontal_wrapped(|ui| {
            for preset in AircraftPreset::ALL {
                ui.label(
                    egui::RichText::new(preset.label())
                        .small()
                        .color(animation_ui::preset_color(preset)),
                );
            }
        });
        ui.checkbox(
            &mut self.anim_editor.show_trail,
            "Afficher la trajectoire simulée (jaune)",
        );
        ui.horizontal(|ui| {
            if ui.button("Vue caméra à t").clicked() {
                self.enter_animation_camera(ctx);
            }
            if ui.button("Vue design").clicked() {
                self.enter_animation_design(ctx);
            }
        });
        let header = egui::CollapsingHeader::new("Réglages fins du profil")
            .default_open(false)
            .show(ui, |ui| {
                let body = &mut self.project.rig.body;
                let mut changed = false;
                changed |= param_f32(
                    ui,
                    "Vitesse max",
                    "m/s",
                    &mut body.max_speed_mps,
                    0.5,
                    300.0,
                    30.0,
                );
                changed |= param_f32(
                    ui,
                    "Accél. avant",
                    "m/s²",
                    &mut body.accel_forward_mps2,
                    0.05,
                    200.0,
                    25.0,
                );
                changed |= param_f32(
                    ui,
                    "Accél. latérale",
                    "m/s²",
                    &mut body.accel_lateral_mps2,
                    0.05,
                    200.0,
                    30.0,
                );
                changed |= param_f32(
                    ui,
                    "Accél. verticale",
                    "m/s²",
                    &mut body.accel_vertical_mps2,
                    0.05,
                    200.0,
                    20.0,
                );
                changed |= param_f32(ui, "Jerk", "m/s³", &mut body.jerk_mps3, 0.1, 1000.0, 60.0);
                changed |= param_f32(ui, "Traînée", "", &mut body.drag, 0.0, 1.0, 0.12);
                changed |= param_f32(ui, "Gain de roulis", "", &mut body.bank_gain, 0.0, 2.0, 1.0);
                changed |= param_f32(
                    ui,
                    "Roulis max",
                    "°",
                    &mut body.max_bank_deg,
                    0.0,
                    89.0,
                    75.0,
                );
                changed |= param_f32(
                    ui,
                    "Réponse roulis",
                    "",
                    &mut body.roll_response,
                    0.1,
                    40.0,
                    10.0,
                );
                changed |= param_f32(ui, "Secousses", "°", &mut body.shake_deg, 0.0, 5.0, 0.1);
                changed |= param_f32(ui, "Turbulence", "", &mut body.turbulence, 0.0, 1.0, 0.35);
                if changed {
                    self.mark_animation_changed();
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Profile,
        );
    }

    fn anim_preview_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(
            egui::RichText::new(
                "Étape 4 · Lisez l'animation dans le viewport, ou rendez la séquence PNG complète.",
            )
            .small(),
        );
        let duration = self.project.rig.duration_s.max(0.1);
        let mut time = self.rig_time.clamp(0.0, duration);
        if ui
            .add(
                egui::Slider::new(&mut time, 0.0..=duration)
                    .text("Temps")
                    .fixed_decimals(2),
            )
            .changed()
        {
            self.anim_playing = false;
            self.rig_time = time;
            self.anim_live_sample = None;
            self.enter_animation_camera(ctx);
        }
        ui.horizontal_wrapped(|ui| {
            let play_label = if self.anim_playing {
                "⏸ Pause"
            } else {
                "▶ Lecture"
            };
            if ui.button(play_label).clicked() {
                self.toggle_animation_play(ctx);
            }
            ui.checkbox(&mut self.anim_loop, "Boucle");
            ui.label("Vitesse");
            ui.add(
                egui::DragValue::new(&mut self.anim_play_speed)
                    .speed(0.05)
                    .range(0.05..=4.0)
                    .suffix("×"),
            );
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("Vue design").clicked() {
                self.enter_animation_design(ctx);
            }
            if ui.button("Vue caméra").clicked() {
                self.enter_animation_camera(ctx);
            }
            if ui.button("Image à ce temps (CPU)").clicked() {
                self.enter_animation_camera(ctx);
                self.start_render(ctx.clone(), true);
            }
        });
        ui.label(
            egui::RichText::new(format!(
                "{} images · {:.2}s · {} fps · espace = lecture/pause",
                self.project.rig.frame_count(),
                self.project.rig.duration_s,
                self.project.rig.fps
            ))
            .small()
            .weak(),
        );
        ui.separator();
        let export_title = ui.label(egui::RichText::new("Séquence PNG").strong());
        if export_title.hovered() {
            self.help
                .register(export_title.rect, help::HelpTopic::Export);
        }
        ui.horizontal(|ui| {
            ui.label("Dossier");
            let folder = ui.add(
                egui::TextEdit::singleline(&mut self.anim_export_dir)
                    .desired_width(ui.available_width() - 8.0),
            );
            if folder.hovered() {
                self.help.register(folder.rect, help::HelpTopic::Export);
            }
        });
        let frames = self.project.rig.frame_count();
        let frames_label = ui.label(
            egui::RichText::new(format!(
                "{} images en {}×{} ({} qualité). Le rendu final CPU peut être long.",
                frames,
                self.render_width,
                self.render_height,
                quality_label(self.quality)
            ))
            .small()
            .weak(),
        );
        if frames_label.hovered() {
            self.help
                .register(frames_label.rect, help::HelpTopic::Export);
        }
        let exporting = self.anim_export_task.is_some();
        ui.horizontal(|ui| {
            let render_sequence =
                ui.add_enabled(!exporting, egui::Button::new("Rendre la séquence"));
            if render_sequence.hovered() {
                self.help
                    .register(render_sequence.rect, help::HelpTopic::Export);
            }
            if render_sequence.clicked() {
                self.start_animation_export(ctx);
            }
            if let Some(task) = &self.anim_export_task {
                let progress = if task.total == 0 {
                    0.0
                } else {
                    task.done as f32 / task.total as f32
                };
                ui.add(
                    egui::ProgressBar::new(progress)
                        .text(format!("{}/{}", task.done, task.total))
                        .desired_width(150.0),
                );
                if ui.button("Annuler").clicked() {
                    task.cancel.store(true, Ordering::Relaxed);
                }
                ui.spinner();
            }
        });
        ui.label(
            egui::RichText::new(
                "Astuce : l'encodeur MP4 reste disponible en CLI (`vistaforge anim render ... --video`).",
            )
            .small()
            .weak(),
        );
        ui.separator();
        let animation_io = egui::CollapsingHeader::new("Trajectoire seule (sans géométrie)")
            .default_open(false)
            .show(ui, |ui| {
                self.animation_io_ui(ui, ctx);
            });
        self.help.register(
            animation_io.header_response.rect.union(
                animation_io
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Export,
        );
    }

    fn start_animation_export(&mut self, ctx: &egui::Context) {
        if self.anim_export_task.is_some() {
            return;
        }
        if let Err(error) = self.project.rig.validate() {
            self.rig_status = format!("Animation invalide : {error}");
            return;
        }
        let out_dir = PathBuf::from(self.anim_export_dir.trim());
        if out_dir.as_os_str().is_empty() {
            self.rig_status = "Choisissez un dossier de sortie".into();
            return;
        }
        let rig = self.project.rig.clone();
        let total = rig.frame_count();
        let mut project = self.project.clone();
        let heightfield = Arc::clone(&self.heightfield);
        let options = self.full_options();
        let seed = self.project.seed;
        let fps = rig.fps.max(1);
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_worker = Arc::clone(&cancel);
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            if let Err(error) = std::fs::create_dir_all(&out_dir) {
                let _ = sender.send(AnimationExportMsg::Failed(format!(
                    "création du dossier : {error}"
                )));
                repaint.request_repaint();
                return;
            }
            let mut simulation = match vf_flight::rig::RigSimulation::new(&rig, &heightfield, seed)
            {
                Ok(simulation) => simulation,
                Err(error) => {
                    let _ = sender.send(AnimationExportMsg::Failed(error));
                    repaint.request_repaint();
                    return;
                }
            };
            let mut written = 0usize;
            let started = Instant::now();
            for index in 0..total {
                if cancel_worker.load(Ordering::Relaxed) {
                    let _ = sender.send(AnimationExportMsg::Cancelled);
                    repaint.request_repaint();
                    return;
                }
                let time = index as f32 / fps as f32;
                let sample = simulation.sample_at(time);
                project.camera = sample.camera;
                match render_with_options(&project, &heightfield, &options) {
                    Ok(output) => {
                        let path = out_dir.join(format!("frame_{index:05}.png"));
                        if let Err(error) = output.save_png(&path) {
                            let _ = sender.send(AnimationExportMsg::Failed(format!(
                                "écriture {} : {error}",
                                path.display()
                            )));
                            repaint.request_repaint();
                            return;
                        }
                        written += 1;
                    }
                    Err(error) => {
                        let _ = sender.send(AnimationExportMsg::Failed(error.to_string()));
                        repaint.request_repaint();
                        return;
                    }
                }
                let _ = sender.send(AnimationExportMsg::Progress {
                    done: index + 1,
                    total,
                });
                if index.is_multiple_of(4) {
                    repaint.request_repaint();
                }
            }
            let _ = sender.send(AnimationExportMsg::Finished {
                written,
                total,
                elapsed: started.elapsed(),
            });
            repaint.request_repaint();
        });
        self.anim_export_task = Some(AnimationExportTask {
            receiver,
            cancel,
            started: Instant::now(),
            total,
            done: 0,
        });
        self.rig_status = format!("Rendu de {total} images…");
        ctx.request_repaint();
    }

    fn poll_animation_export(&mut self, ctx: &egui::Context) {
        let Some(task) = self.anim_export_task.as_mut() else {
            return;
        };
        let mut finished = false;
        let mut messages = Vec::new();
        loop {
            match task.receiver.try_recv() {
                Ok(message) => messages.push(message),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    finished = true;
                    break;
                }
            }
        }
        for message in messages {
            match message {
                AnimationExportMsg::Progress { done, total } => {
                    task.done = done;
                    task.total = total;
                    if done.is_multiple_of(8) {
                        self.rig_status = format!("Séquence : {done}/{total} images");
                    }
                }
                AnimationExportMsg::Finished {
                    written,
                    total,
                    elapsed,
                } => {
                    self.rig_status = format!(
                        "Séquence terminée : {written}/{total} images dans {} en {:.1}s",
                        self.anim_export_dir,
                        elapsed.as_secs_f32()
                    );
                    finished = true;
                }
                AnimationExportMsg::Cancelled => {
                    self.rig_status = "Séquence annulée".into();
                    finished = true;
                }
                AnimationExportMsg::Failed(error) => {
                    self.rig_status = format!("Séquence : {error}");
                    finished = true;
                }
            }
        }
        if finished {
            self.anim_export_task = None;
        } else if let Some(task) = &self.anim_export_task {
            let _ = task.started;
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }

    fn help_workspace_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(
            egui::RichText::new(
                "Toute la documentation est ici. Le survol n'est pas nécessaire : choisissez une fiche à gauche, lisez-la dans le panneau central, puis revenez au terrain quand vous voulez.",
            )
            .size(11.0)
            .color(egui::Color32::from_rgb(188, 202, 220)),
        );
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("Retour au terrain")
                .on_hover_text("Revenir à l'espace Terrain sans perdre la fiche choisie")
                .clicked()
            {
                self.workspace = Workspace::Terrain;
            }
            if ui.button("Préférences").clicked() {
                self.preferences_open = true;
            }
            ui.label(
                egui::RichText::new("Aide flottante : bouton de la barre d'outils ou F1")
                    .small()
                    .weak(),
            );
        });
        ui.separator();
        self.help.browse_index_ui(ui);
        let _ = ctx;
    }

    fn workspace_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if self.workspace == Workspace::Help {
            self.help_workspace_panel(ui, ctx);
            return;
        }
        self.help.register_front(
            ui.available_rect_before_wrap(),
            help_topic_for_workspace(self.workspace),
        );
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                self.preview_quality_picker(ui);
                if ui
                    .button("Aperçu")
                    .on_hover_text(
                        "Relance l'aperçu CPU à la qualité choisie. « Finale » rend à la résolution de sortie.",
                    )
                    .clicked()
                {
                    self.start_quality_preview(ctx.clone());
                }
                if ui
                    .selectable_label(self.compare.enabled, "Avant/Après")
                    .on_hover_text(
                        "Compare le rendu CPU figé à l'état courant, directement depuis ce panneau.",
                    )
                    .clicked()
                {
                    if self.compare.enabled {
                        self.compare_apply(ctx);
                    } else {
                        self.cpu_preview_forced = true;
                        self.compare_start(ctx);
                    }
                }
            });
            ui.separator();
            match self.workspace {
                Workspace::Help => {}
                Workspace::Terrain => self.terrain_ui(ui, ctx),
                Workspace::Modifiers => self.modifiers_ui(ui, ctx),
                Workspace::Rock => self.rock_ui(ui, ctx),
                Workspace::Detail => self.detail_ui(ui, ctx),
                Workspace::Sculpt => self.sculpt_panel_ui(ui, ctx),
                Workspace::Animation => self.animation_panel_ui(ui, ctx),
                Workspace::Erosion => self.erosion_ui(ui),
                Workspace::Water => self.water_ui(ui),
                Workspace::Materials => self.materials_ui(ui),
                Workspace::Light => self.light_ui(ui),
                Workspace::Camera => self.camera_ui(ui),
                Workspace::Render => self.render_ui(ui, ctx),
                Workspace::Files => self.files_ui(ui, ctx),
            }
            ui.add_space(12.0);
            let stats = self.terrain_stats();
            ui.label(
                egui::RichText::new(format!(
                    "{}×{} · relief {:.0} m · {} échantillons",
                    stats.width,
                    stats.height,
                    stats.relief(),
                    stats.valid_cells
                ))
                .small()
                .weak(),
            );
        });
    }

    fn apply_file_pick(&mut self, pick: file_picker::PickResult) {
        let path = pick.path.display().to_string();
        match pick.target {
            file_picker::PickTarget::Project => self.path_input = path,
            file_picker::PickTarget::ImportTerrain => self.files_import_path = path,
            file_picker::PickTarget::ExportTerrain => self.files_export_path = path,
            file_picker::PickTarget::Animation => self.anim_io_path = path,
            file_picker::PickTarget::BlenderDir => self.blender_export_dir = path,
            file_picker::PickTarget::Texture => {
                self.project.materials.texture_path = path;
                self.mark_dirty();
            }
        }
    }

    fn files_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.label(
            egui::RichText::new(
                "Échange avec le reste de la chaîne : GeoTIFF/ASCII pour les DEM, PNG/JPEG pour les heightmaps, glTF/GLB pour Blender, .vfcam/GLB/CSV pour les trajectoires seules.",
            )
            .small(),
        );

        let project_header = egui::CollapsingHeader::new("Projet")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Fichier");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.path_input)
                            .desired_width(ui.available_width() - 40.0)
                            .hint_text("monde.vfproj"),
                    );
                    if ui.small_button("…").clicked() {
                        let current = self.path_input.clone();
                        self.file_picker.open(
                            file_picker::PickTarget::Project,
                            file_picker::PickMode::OpenFile,
                            &current,
                            &["vfproj"],
                        );
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Nouveau").clicked() {
                        self.new_project(ctx.clone());
                    }
                    if ui.button("Ouvrir").clicked() {
                        self.open_project(ctx.clone());
                    }
                    if ui.button("Enregistrer").clicked() {
                        self.save_project(ctx);
                    }
                    if let Some(path) = &self.project_path {
                        ui.label(
                            egui::RichText::new(format!("actuel : {}", path.display()))
                                .small()
                                .weak(),
                        );
                    }
                });
                ui.label(
                    egui::RichText::new(
                        "Format de projet .vfproj : ZIP + manifeste RON + heightmap f32 (aucune perte). Ctrl+S / Ctrl+O.",
                    )
                    .small()
                    .weak(),
                );
            });
        self.help.register(
            project_header.header_response.rect.union(
                project_header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Export,
        );

        let import_header = egui::CollapsingHeader::new("Importer un terrain")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Fichier");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.files_import_path)
                            .desired_width(ui.available_width() - 40.0)
                            .hint_text("heightmap.png · dem.tif · terrain.asc · vieux.dem"),
                    );
                    if ui.small_button("…").clicked() {
                        let current = self.files_import_path.clone();
                        self.file_picker.open(
                            file_picker::PickTarget::ImportTerrain,
                            file_picker::PickMode::OpenFile,
                            &current,
                            &["tif", "tiff", "png", "jpg", "jpeg", "bmp", "asc", "grd", "dem", "r32"],
                        );
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Élévation");
                    ui.add(
                        egui::DragValue::new(&mut self.files_min_height)
                            .speed(10.0)
                            .prefix("min "),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.files_max_height)
                            .speed(10.0)
                            .prefix("max "),
                    );
                    ui.label(egui::RichText::new("0/0 = auto").small().weak());
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Cellule");
                    ui.add(
                        egui::DragValue::new(&mut self.files_spacing)
                            .speed(0.5)
                            .range(0.0..=10_000.0)
                            .suffix(" m"),
                    );
                    ui.label(egui::RichText::new("0 = géoréf. / 1 m").small().weak());
                    ui.checkbox(&mut self.files_flip_y, "Lignes nord en haut");
                    ui.checkbox(
                        &mut self.files_invert_height,
                        "Inverser les hauteurs (noir = haut)",
                    )
                    .on_hover_text(
                        "Pour les heightmaps ou scans où le blanc représente le point bas. L'aperçu et l'export restent blanc = haut.",
                    );
                });
                ui.label(
                    egui::RichText::new(
                        "PNG/JPEG : image en niveaux de gris, la plage min/max ou le sidecar JSON fixe l'altitude. TIFF : GeoTIFF 32 bits lu avec son géoréférencement.",
                    )
                    .small()
                    .weak(),
                );
                if ui
                    .add_sized([ui.available_width(), 28.0], egui::Button::new("Importer le terrain"))
                    .clicked()
                {
                    self.files_import_terrain(ctx);
                }
            });
        self.help.register(
            import_header.header_response.rect.union(
                import_header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Export,
        );

        let export_header = egui::CollapsingHeader::new("Exporter le terrain")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Fichier");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.files_export_path)
                            .desired_width(ui.available_width() - 40.0)
                            .hint_text("dem.tif · dem.asc · height.png · height.jpg · vieux.dem"),
                    );
                    if ui.small_button("…").clicked() {
                        let current = self.files_export_path.clone();
                        self.file_picker.open(
                            file_picker::PickTarget::ExportTerrain,
                            file_picker::PickMode::SaveFile,
                            &current,
                            &[
                                "tif", "tiff", "asc", "grd", "png", "jpg", "jpeg", "dem", "r32",
                            ],
                        );
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Nodata");
                    ui.add(egui::DragValue::new(&mut self.files_nodata).speed(1.0));
                    ui.label(
                        egui::RichText::new(
                            ".tif = GeoTIFF float · .asc = ESRI · .png = 16 bits + JSON",
                        )
                        .small()
                        .weak(),
                    );
                });
                if ui
                    .add_sized(
                        [ui.available_width(), 28.0],
                        egui::Button::new("Exporter le terrain"),
                    )
                    .clicked()
                {
                    self.files_export_terrain(ctx);
                }
            });
        self.help.register(
            export_header.header_response.rect.union(
                export_header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Export,
        );

        let blender_header = egui::CollapsingHeader::new("Blender")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Dossier");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.blender_export_dir)
                            .desired_width(ui.available_width() - 40.0),
                    );
                    if ui.small_button("…").clicked() {
                        let current = self.blender_export_dir.clone();
                        self.file_picker.open(
                            file_picker::PickTarget::BlenderDir,
                            file_picker::PickMode::Directory,
                            &current,
                            &[],
                        );
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Subdivision");
                    ui.add(
                        egui::DragValue::new(&mut self.blender_subdivision)
                            .speed(1.0)
                            .range(1..=4),
                    );
                    ui.checkbox(
                        &mut self.blender_export_animation,
                        "camera_animation.glb",
                    )
                    .on_hover_text(
                        "Écrit aussi la trajectoire caméra/cible dans le dossier, pour l'addon Blender.",
                    );
                    ui.label(
                        egui::RichText::new("scene.glb + textures + scene.json")
                            .small()
                            .weak(),
                    );
                });
                if self
                    .task
                    .as_ref()
                    .is_some_and(|task| task.kind == jobs::TaskKind::Blender)
                {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Export Blender en cours…");
                    });
                } else if ui
                    .add_sized(
                        [ui.available_width(), 28.0],
                        egui::Button::new("Exporter pour Blender (GLB)"),
                    )
                    .clicked()
                {
                    self.files_export_blender(ctx);
                }
                ui.label(
                    egui::RichText::new(
                        "Importer dans Blender : File > Import > glTF 2.0, ou l'addon bridge/blender/vistaforge_io.py pour les trajectoires.",
                    )
                    .small()
                    .weak(),
                );
            });
        self.help.register(
            blender_header.header_response.rect.union(
                blender_header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Export,
        );

        let animation_header = egui::CollapsingHeader::new("Animation caméra / cible")
            .default_open(true)
            .show(ui, |ui| {
                self.animation_io_ui(ui, ctx);
            });
        self.help.register(
            animation_header.header_response.rect.union(
                animation_header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Export,
        );

        ui.separator();
        ui.label(
            egui::RichText::new(&self.files_status)
                .size(10.5)
                .color(egui::Color32::from_rgb(180, 200, 220)),
        );
    }

    fn files_import_terrain(&mut self, ctx: &egui::Context) {
        let path = PathBuf::from(self.files_import_path.trim());
        if path.as_os_str().is_empty() {
            self.files_status = "Import : chemin vide".into();
            return;
        }
        let options = vf_io::TerrainImportOptions {
            heightmap: vf_io::HeightmapOptions {
                min_height: self.files_min_height,
                max_height: self.files_max_height,
                spacing_m: f64::from(self.files_spacing),
                origin: [0.0, 0.0],
                flip_y: self.files_flip_y,
                invert_height: self.files_invert_height,
            },
            dem_spacing_m: if self.files_spacing > 0.0 {
                self.files_spacing
            } else {
                30.0
            },
        };
        let result_path = path.clone();
        let title = format!(
            "Import du terrain {}…",
            path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned()
            )
        );
        if self.start_task(
            ctx,
            jobs::TaskKind::ImportTerrain,
            title.clone(),
            move |progress| {
                progress.set_phase("Lecture du fichier et conversion des altitudes…");
                progress.set_indeterminate();
                match vf_io::import_terrain(&path, options) {
                    Ok(imported) => {
                        progress.set(1.0);
                        jobs::TaskResult::TerrainImported {
                            path: result_path,
                            imported,
                        }
                    }
                    Err(error) => jobs::TaskResult::Failed(format!("Import impossible : {error}")),
                }
            },
        ) {
            self.files_status = format!("{title} (l'interface reste utilisable)");
            self.status = self.files_status.clone();
        }
    }

    fn files_export_terrain(&mut self, ctx: &egui::Context) {
        let path = PathBuf::from(self.files_export_path.trim());
        if path.as_os_str().is_empty() {
            self.files_status = "Export : chemin vide".into();
            return;
        }
        let heightfield = Arc::clone(&self.heightfield);
        let name = self.project.meta.name.clone();
        let nodata = self.files_nodata;
        let title = format!(
            "Export du terrain vers {}…",
            path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned()
            )
        );
        if self.start_task(
            ctx,
            jobs::TaskKind::ExportTerrain,
            title.clone(),
            move |progress| {
                progress.set_phase("Conversion et écriture du fichier…");
                progress.set_indeterminate();
                match vf_io::export_terrain(&path, &heightfield, &name, Some(nodata)) {
                    Ok(report) => {
                        progress.set(1.0);
                        jobs::TaskResult::TerrainExported { report }
                    }
                    Err(error) => jobs::TaskResult::Failed(format!("Export impossible : {error}")),
                }
            },
        ) {
            self.files_status = format!("{title} (l'interface reste utilisable)");
            self.status = self.files_status.clone();
        }
    }

    fn files_export_blender(&mut self, ctx: &egui::Context) {
        let out_dir = PathBuf::from(self.blender_export_dir.trim());
        if out_dir.as_os_str().is_empty() {
            self.files_status = "Blender : dossier vide".into();
            return;
        }
        let project = self.project.clone();
        let heightfield = Arc::clone(&self.heightfield);
        let subdivision = self.blender_subdivision;
        let include_animation = self.blender_export_animation;
        let status_dir = out_dir.display().to_string();
        let title = format!("Export Blender vers {status_dir}…");
        if self.start_task(
            ctx,
            jobs::TaskKind::Blender,
            title.clone(),
            move |progress| {
                progress.set_phase("Construction du maillage et des textures…");
                progress.set_indeterminate();
                let options = vf_export::ExportOptions {
                    subdivision,
                    ..vf_export::ExportOptions::default()
                };
                let report =
                    match vf_export::export_project(&project, &heightfield, &out_dir, &options) {
                        Ok(report) => report,
                        Err(error) => {
                            return jobs::TaskResult::Failed(format!("Blender : échec — {error}"));
                        }
                    };
                let mut note = None;
                if include_animation && project.rig.enabled {
                    progress.set_phase("Export de la trajectoire caméra…");
                    progress.set(0.85);
                    match vf_export::export_animation(
                        &out_dir.join("camera_animation.glb"),
                        &project.rig,
                        &heightfield,
                        project.seed,
                    ) {
                        Ok(animation) => {
                            note = Some(format!(
                                "trajectoire {} images → camera_animation.glb",
                                animation.frames
                            ));
                        }
                        Err(error) => {
                            note = Some(format!("trajectoire non exportée : {error}"));
                        }
                    }
                }
                progress.set(1.0);
                jobs::TaskResult::BlenderExported { report, note }
            },
        ) {
            self.files_status = format!("{title} (l'interface reste utilisable)");
            self.status = self.files_status.clone();
        }
    }

    fn animation_io_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.label("Fichier");
            ui.add(
                egui::TextEdit::singleline(&mut self.anim_io_path)
                    .desired_width(ui.available_width() - 40.0)
                    .hint_text("flyover.glb · flyover.vfcam · flyover.csv"),
            );
            if ui.small_button("…").clicked() {
                let current = self.anim_io_path.clone();
                self.file_picker.open(
                    file_picker::PickTarget::Animation,
                    file_picker::PickMode::OpenFile,
                    &current,
                    &["glb", "vfcam", "json", "csv"],
                );
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("Exporter GLB")
                .on_hover_text("Caméra + cible animées, pour Blender ou tout outil glTF")
                .clicked()
            {
                self.export_animation_with_extension(ctx, "glb");
            }
            if ui
                .button("Exporter .vfcam")
                .on_hover_text("Rig complet VistaForge, sans géométrie")
                .clicked()
            {
                self.export_animation_with_extension(ctx, "vfcam");
            }
            if ui
                .button("Exporter CSV")
                .on_hover_text("Une ligne par image, lisible par tableur")
                .clicked()
            {
                self.export_animation_with_extension(ctx, "csv");
            }
            if ui
                .button("Importer une trajectoire")
                .on_hover_text("Remplace le rig courant, conserve le terrain")
                .clicked()
            {
                self.import_animation_file(ctx);
            }
        });
        ui.label(
            egui::RichText::new(
                "Le GLB contient aussi le rig complet dans ses extras : réimport dans VistaForge sans perte. Les fichiers .vfcam/CSV ne contiennent aucune géométrie.",
            )
            .small()
            .weak(),
        );
    }

    fn export_animation_with_extension(&mut self, ctx: &egui::Context, extension: &str) {
        let mut path = PathBuf::from(self.anim_io_path.trim());
        if path.as_os_str().is_empty() {
            self.files_status = "Animation : chemin vide".into();
            return;
        }
        path.set_extension(extension);
        let rig = self.project.rig.clone();
        let heightfield = Arc::clone(&self.heightfield);
        let seed = self.project.seed;
        let title = format!("Export de la trajectoire .{extension}…");
        if self.start_task(
            ctx,
            jobs::TaskKind::AnimationExport,
            title.clone(),
            move |progress| {
                progress.set_phase("Échantillonnage des images et écriture du fichier…");
                progress.set_indeterminate();
                match vf_export::export_animation(&path, &rig, &heightfield, seed) {
                    Ok(report) => {
                        progress.set(1.0);
                        jobs::TaskResult::AnimationExported { report }
                    }
                    Err(error) => {
                        jobs::TaskResult::Failed(format!("Animation : export impossible — {error}"))
                    }
                }
            },
        ) {
            self.files_status = format!("{title} (l'interface reste utilisable)");
            self.rig_status = self.files_status.clone();
            self.status = self.files_status.clone();
        }
    }

    fn import_animation_file(&mut self, ctx: &egui::Context) {
        let path = PathBuf::from(self.anim_io_path.trim());
        if path.as_os_str().is_empty() {
            self.files_status = "Animation : chemin vide".into();
            return;
        }
        let title = "Import de la trajectoire…".to_string();
        if self.start_task(
            ctx,
            jobs::TaskKind::AnimationImport,
            title.clone(),
            move |progress| {
                progress.set_phase("Lecture du fichier et reconstruction du rig…");
                progress.set_indeterminate();
                match vf_export::import_animation(&path) {
                    Ok(imported) => {
                        progress.set(1.0);
                        jobs::TaskResult::AnimationImported { imported }
                    }
                    Err(error) => {
                        jobs::TaskResult::Failed(format!("Animation : import impossible — {error}"))
                    }
                }
            },
        ) {
            self.files_status = format!("{title} (l'interface reste utilisable)");
            self.rig_status = self.files_status.clone();
            self.status = self.files_status.clone();
        }
    }

    fn camera_ui(&mut self, ui: &mut egui::Ui) {
        let header = egui::CollapsingHeader::new("Vues CAO")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for preset in [
                        vf_scene::ViewPreset::Perspective,
                        vf_scene::ViewPreset::Front,
                        vf_scene::ViewPreset::Top,
                        vf_scene::ViewPreset::Right,
                        vf_scene::ViewPreset::Left,
                        vf_scene::ViewPreset::Back,
                        vf_scene::ViewPreset::Bottom,
                    ] {
                        if ui
                            .small_button(preset.label())
                            .on_hover_text("Cadrer le terrain sur cette vue")
                            .clicked()
                        {
                            self.set_view_preset(preset);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Vue par défaut").clicked() {
                        self.home_view();
                    }
                    ui.checkbox(&mut self.show_gizmo, "Gizmo d'orientation");
                });
                ui.label(
                    egui::RichText::new(
                        "Raccourcis : 1 face · 2 dessus · 3 droite · 0/Home défaut",
                    )
                    .small()
                    .weak(),
                );
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Camera,
        );
        let header = egui::CollapsingHeader::new("Caméra")
            .default_open(true)
            .show(ui, |ui| {
                if ui
                    .add(egui::Slider::new(&mut self.yaw, -180.0..=180.0).text("Yaw"))
                    .changed()
                {
                    self.apply_orbit();
                    self.mark_camera_dirty();
                }
                if ui
                    .add(egui::Slider::new(&mut self.pitch, -90.0..=90.0).text("Pitch"))
                    .changed()
                {
                    self.apply_orbit();
                    self.mark_camera_dirty();
                }
                if ui
                    .add(
                        egui::Slider::new(&mut self.distance, 100.0..=80_000.0)
                            .logarithmic(true)
                            .text("Distance (m)"),
                    )
                    .changed()
                {
                    self.apply_orbit();
                    self.mark_camera_dirty();
                }
                if ui
                    .add(egui::Slider::new(&mut self.fov, 10.0..=110.0).text("FOV (°)"))
                    .changed()
                {
                    self.project.camera.fov_y_deg = self.fov;
                    self.apply_orbit();
                    self.mark_camera_dirty();
                }
                ui.label(
                    egui::RichText::new(format!(
                        "orbite {:.0}° / {:.0}° · {:.0} m · eye ({:.0}, {:.0}, {:.0})",
                        self.yaw,
                        self.pitch,
                        self.distance,
                        self.project.camera.position.x,
                        self.project.camera.position.y,
                        self.project.camera.position.z
                    ))
                    .small()
                    .weak(),
                );
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Camera,
        );
    }

    fn render_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let header = egui::CollapsingHeader::new("Output")
            .default_open(true)
            .show(ui, |ui| {
                let mut size_changed = false;
                ui.horizontal(|ui| {
                    ui.label("Size");
                    size_changed |= ui
                        .add(
                            egui::DragValue::new(&mut self.render_width)
                                .speed(8.0)
                                .range(64..=8192),
                        )
                        .changed();
                    ui.label("x");
                    size_changed |= ui
                        .add(
                            egui::DragValue::new(&mut self.render_height)
                                .speed(8.0)
                                .range(64..=8192),
                        )
                        .changed();
                });
                if size_changed {
                    self.project.render.width = self.render_width;
                    self.project.render.height = self.render_height;
                    self.mark_dirty();
                }
                ui.horizontal(|ui| {
                    ui.label("Quality");
                    let mut selected = self.quality;
                    egui::ComboBox::from_id_salt("quality")
                        .selected_text(quality_label(self.quality))
                        .show_ui(ui, |ui| {
                            for quality in
                                [Quality::Low, Quality::Medium, Quality::High, Quality::Ultra]
                            {
                                ui.selectable_value(&mut selected, quality, quality_label(quality));
                            }
                        });
                    if selected != self.quality {
                        self.quality = selected;
                        let (_, _, shadows, ao, _, _) = selected.presets();
                        self.shadows = shadows;
                        self.ao = ao;
                        self.project.render.quality = selected;
                        self.project.render.shadows = shadows;
                        self.project.render.ao = ao;
                        self.project.render.ssaa = selected.presets().4;
                        self.project.render.shadow_resolution = selected.presets().5;
                        self.mark_dirty();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Mode");
                    let mut mode = self.project.render.render_mode;
                    egui::ComboBox::from_id_salt("render-mode")
                        .selected_text(render_mode_label(mode))
                        .show_ui(ui, |ui| {
                            for candidate in
                                [RenderMode::Fast, RenderMode::Refined, RenderMode::Photoreal]
                            {
                                ui.selectable_value(
                                    &mut mode,
                                    candidate,
                                    render_mode_label(candidate),
                                );
                            }
                        });
                    if mode != self.project.render.render_mode {
                        mode.apply(&mut self.project.render);
                        self.quality = Quality::Custom;
                        self.project.render.quality = Quality::Custom;
                        self.shadows = self.project.render.shadows;
                        self.ao = self.project.render.ao;
                        self.mark_dirty();
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    let mut lighting_changed = false;
                    lighting_changed |= ui.checkbox(&mut self.shadows, "Shadows").changed();
                    lighting_changed |= ui.checkbox(&mut self.ao, "AO").changed();
                    if lighting_changed {
                        self.project.render.shadows = self.shadows;
                        self.project.render.ao = self.ao;
                        self.mark_dirty();
                    }
                    let adaptive = ui.checkbox(&mut self.adaptive_preview, "Aperçu adaptatif");
                    if adaptive.hovered() {
                        self.help
                            .register(adaptive.rect, help::HelpTopic::Adaptive);
                    }
                    if adaptive.changed() {
                        if !self.adaptive_preview {
                            self.preview_scale = 1.0;
                            self.refine_at = None;
                        }
                        self.mark_dirty();
                    }
                    if ui
                        .checkbox(&mut self.project.render.adaptive, "LOD adaptatif")
                        .changed()
                    {
                        self.mark_dirty();
                    }
                    if ui
                        .checkbox(&mut self.project.render.cliffs, "Cliff mesh")
                        .changed()
                    {
                        self.mark_dirty();
                    }
                    let mut scene_changed = false;
                    scene_changed |= ui.checkbox(&mut self.clouds, "Clouds").changed();
                    scene_changed |= ui.checkbox(&mut self.water, "Water").changed();
                    if scene_changed {
                        self.project.render.clouds = self.clouds;
                        self.project.render.water = self.water;
                        self.mark_dirty();
                    }
                });
                let gpu_info = self
                    .gpu
                    .as_ref()
                    .map(|gpu| (gpu.label().to_owned(), gpu.summary()));
                if let Some((label, summary)) = gpu_info {
                    let mut enabled = self.gpu_enabled;
                    let gpu_check = ui.checkbox(&mut enabled, format!("Vue 3D GPU — {label}"));
                    if gpu_check.hovered() {
                        self.help.register(gpu_check.rect, help::HelpTopic::Gpu);
                    }
                    if gpu_check.changed() {
                        self.gpu_enabled = enabled;
                        self.mark_dirty();
                        if enabled {
                            self.gpu_mesh_dirty = true;
                            self.gpu_mesh_due = Some(Instant::now());
                        }
                    }
                    ui.label(egui::RichText::new(summary).small().weak());
                    if self.gpu_enabled
                        && self.show_final_render
                        && ui.button("Revenir à la vue GPU").clicked()
                    {
                        self.show_final_render = false;
                    }
                } else if let Some(error) = &self.gpu_error {
                    ui.label(
                        egui::RichText::new(format!("GPU indisponible : {error}"))
                            .small()
                            .weak(),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(
                            "Vue GPU indisponible : démarrage en mode OpenGL (VISTAFORGE_RENDERER=wgpu pour l'activer).",
                        )
                        .small()
                        .weak(),
                    );
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_sized(
                            [ui.available_width() * 0.55, 28.0],
                            egui::Button::new("Render"),
                        )
                        .on_hover_text("Rendu final CPU déterministe, à la résolution de sortie")
                        .clicked()
                    {
                        self.start_render(ctx.clone(), true);
                    }
                    if ui
                        .button("Preview")
                        .on_hover_text("Aperçu CPU à la qualité choisie dans le panneau")
                        .clicked()
                    {
                        self.start_quality_preview(ctx.clone());
                    }
                });
                if self.render_task.as_ref().is_some_and(|task| task.full) {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("rendering…");
                    });
                }
                ui.label(
                    egui::RichText::new(format!(
                        "dernier rendu : {}×{} · {} ms",
                        self.last_render_size.0, self.last_render_size.1, self.last_render_ms
                    ))
                    .small()
                    .weak(),
                );
                let preview_w = ((PREVIEW_SIZE.0 as f32) * self.preview_scale)
                    .round()
                    .max(160.0) as u32;
                let preview_h = ((PREVIEW_SIZE.1 as f32) * self.preview_scale)
                    .round()
                    .max(90.0) as u32;
                ui.label(
                    egui::RichText::new(format!(
                        "aperçu adaptatif : {preview_w}×{preview_h} · {} ms · échelle {:.0} % (cible {} ms)",
                        self.preview_ms,
                        self.preview_scale * 100.0,
                        PREVIEW_TARGET_MS
                    ))
                    .small()
                    .weak(),
                );
                ui.separator();
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.render_path).desired_width(180.0));
                    if ui.button("Save PNG").clicked() {
                        self.save_render(ctx);
                    }
                });
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::RenderFinal,
        );
    }

    fn sculpt_panel_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let (sx, sy) = self.heightfield.spacing();
        let cell_m = sx.max(sy);
        let header = egui::CollapsingHeader::new("Définition")
            .default_open(true)
            .show(ui, |ui| {
                let relief = self.terrain_stats().relief();
                ui.label(
                    egui::RichText::new(format!(
                        "{}×{} · cellule {:.1} m · relief {:.0} m",
                        self.heightfield.width(),
                        self.heightfield.height(),
                        cell_m,
                        relief
                    ))
                    .small(),
                );
                ui.horizontal(|ui| {
                    ui.label("Aperçu");
                    let mut detail = self.sculpt_detail;
                    egui::ComboBox::from_id_salt("sculpt-detail")
                        .selected_text(format!("{}×", self.sculpt_preview_factor()))
                        .show_ui(ui, |ui| {
                            for value in 1..=3 {
                                ui.selectable_value(&mut detail, value, format!("{value}×"));
                            }
                        });
                    if detail != self.sculpt_detail {
                        self.sculpt_detail = detail;
                        self.sculpt_texture_dirty = true;
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "{}×{} px",
                            self.heightfield.width() * self.sculpt_preview_factor() as usize,
                            self.heightfield.height() * self.sculpt_preview_factor() as usize
                        ))
                        .small()
                        .weak(),
                    );
                });
                if param_f32(
                    ui,
                    "Exagération",
                    "×",
                    &mut self.sculpt_exaggeration,
                    0.5,
                    4.0,
                    1.8,
                ) {
                    self.sculpt_texture_dirty = true;
                }
                if param_f32(
                    ui,
                    "Courbes de niveau",
                    "m",
                    &mut self.sculpt_contours_m,
                    0.0,
                    100.0,
                    5.0,
                ) {
                    self.sculpt_texture_dirty = true;
                }
                ui.label(
                    egui::RichText::new(
                        "Les courbes révèlent les bancs fins ; 0 les désactive.",
                    )
                    .small()
                    .weak(),
                );
                let max_size = self.heightfield.width().max(self.heightfield.height());
                if max_size < 4096 {
                    if ui
                        .button(format!(
                            "Densifier ×2 (→ {:.1} m/cellule)",
                            cell_m * 0.5
                        ))
                        .on_hover_text(
                            "Rééchantillonne le terrain en bicubique. Indispensable pour des bancs de quelques mètres ; l'historique de sculpture est vidé.",
                        )
                        .clicked()
                    {
                        self.densify_sculpt_terrain(ctx);
                    }
                } else {
                    ui.label(egui::RichText::new("Définition maximale (4096²).").small());
                }
                if cell_m > 8.0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "Attention : cellule de {:.1} m. Les terrasses fines demandent ≤ 8 m/cellule ; densifiez avant de sculpter les bancs.",
                            cell_m
                        ))
                        .size(10.0)
                        .color(egui::Color32::from_rgb(255, 196, 110)),
                    );
                }
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Sculpt,
        );

        let header = egui::CollapsingHeader::new("Pinceau")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Mode");
                    let previous_mode = self.sculpt_brush.mode;
                    egui::ComboBox::from_id_salt("sculpt-mode")
                        .selected_text(brush_mode_label(self.sculpt_brush.mode))
                        .show_ui(ui, |ui| {
                            for mode in [
                                BrushMode::Raise,
                                BrushMode::Lower,
                                BrushMode::Smooth,
                                BrushMode::Flatten,
                                BrushMode::Terrace,
                                BrushMode::Ridge,
                                BrushMode::Groove,
                                BrushMode::Crater,
                                BrushMode::Noise,
                                BrushMode::Warp,
                                BrushMode::Clone,
                            ] {
                                ui.selectable_value(
                                    &mut self.sculpt_brush.mode,
                                    mode,
                                    brush_mode_label(mode),
                                );
                            }
                        });
                    if self.sculpt_brush.mode != previous_mode {
                        match self.sculpt_brush.mode {
                            BrushMode::Terrace => {
                                self.sculpt_brush.strength = 1.0;
                                self.sculpt_brush.hardness = 0.55;
                                self.sculpt_brush.roughness = 0.12;
                                self.sculpt_status =
                                    "Terrasse : réglez l'épaisseur des bancs et densifiez si la cellule est trop grande."
                                        .into();
                            }
                            BrushMode::Groove => {
                                self.sculpt_brush.strength = 4.0;
                                self.sculpt_brush.hardness = 0.5;
                                self.sculpt_brush.roughness = 0.15;
                                self.sculpt_status =
                                    "Orgues : espacement/profondeur, puis trait le long de la pente."
                                        .into();
                            }
                            BrushMode::Smooth | BrushMode::Flatten | BrushMode::Clone => {
                                self.sculpt_brush.strength = 0.5;
                            }
                            BrushMode::Warp => self.sculpt_brush.strength = 20.0,
                            _ => {
                                if self.sculpt_brush.strength <= 1.0 {
                                    self.sculpt_brush.strength = 8.0;
                                }
                            }
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Forme");
                    egui::ComboBox::from_id_salt("sculpt-shape")
                        .selected_text(brush_shape_label(self.sculpt_brush.shape))
                        .show_ui(ui, |ui| {
                            for shape in [
                                BrushShape::Circle,
                                BrushShape::Square,
                                BrushShape::Diamond,
                                BrushShape::Ring,
                                BrushShape::Star,
                                BrushShape::Polygon,
                            ] {
                                ui.selectable_value(
                                    &mut self.sculpt_brush.shape,
                                    shape,
                                    brush_shape_label(shape),
                                );
                            }
                        });
                });
                if self.sculpt_brush.shape == BrushShape::Polygon {
                    let mut sides = self.sculpt_brush.sides as f32;
                    if param_f32(ui, "Côtés", "", &mut sides, 3.0, 24.0, 6.0) {
                        self.sculpt_brush.sides = sides.round() as u32;
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Retombée");
                    egui::ComboBox::from_id_salt("sculpt-falloff")
                        .selected_text(falloff_label(self.sculpt_brush.falloff))
                        .show_ui(ui, |ui| {
                            for falloff in [
                                Falloff::Constant,
                                Falloff::Linear,
                                Falloff::Smooth,
                                Falloff::Gaussian,
                                Falloff::Plateau,
                            ] {
                                ui.selectable_value(
                                    &mut self.sculpt_brush.falloff,
                                    falloff,
                                    falloff_label(falloff),
                                );
                            }
                        });
                });
                param_f32(
                    ui,
                    "Taille",
                    "m",
                    &mut self.sculpt_brush.radius_m,
                    0.1,
                    2000.0,
                    80.0,
                );
                param_f32(
                    ui,
                    "Aspect",
                    "",
                    &mut self.sculpt_brush.aspect,
                    0.1,
                    10.0,
                    1.0,
                );
                param_f32(
                    ui,
                    "Angle",
                    "°",
                    &mut self.sculpt_brush.angle_deg,
                    -180.0,
                    180.0,
                    0.0,
                );
                let (strength_label, strength_unit, strength_max, strength_default) =
                    match self.sculpt_brush.mode {
                        BrushMode::Terrace
                        | BrushMode::Smooth
                        | BrushMode::Flatten
                        | BrushMode::Clone => ("Mélange", "", 1.0, 1.0),
                        BrushMode::Groove => ("Profondeur", "m", 100.0, 4.0),
                        BrushMode::Warp => ("Amplitude", "m", 100.0, 20.0),
                        _ => ("Intensité", "m", 100.0, 8.0),
                    };
                param_f32(
                    ui,
                    strength_label,
                    strength_unit,
                    &mut self.sculpt_brush.strength,
                    0.0,
                    strength_max,
                    strength_default,
                );
                param_f32(
                    ui,
                    "Dureté",
                    "",
                    &mut self.sculpt_brush.hardness,
                    0.0,
                    1.0,
                    0.35,
                );
                param_f32(
                    ui,
                    "Rugosité",
                    "",
                    &mut self.sculpt_brush.roughness,
                    0.0,
                    1.0,
                    0.25,
                );
                param_f32(
                    ui,
                    "Échelle du bruit",
                    "m",
                    &mut self.sculpt_brush.noise_scale_m,
                    0.1,
                    1000.0,
                    12.0,
                );
                let mut aa = self.sculpt_brush.aa_samples as f32;
                if param_f32(ui, "Anti-aliasing", "×", &mut aa, 1.0, 8.0, 4.0) {
                    self.sculpt_brush.aa_samples = aa.round().clamp(1.0, 8.0) as u32;
                }
                if self.sculpt_brush.mode == BrushMode::Terrace {
                    param_f32(
                        ui,
                        "Épaisseur des bancs",
                        "m",
                        &mut self.sculpt_brush.step_m,
                        0.1,
                        500.0,
                        25.0,
                    );
                    egui::CollapsingHeader::new("Strates et colonnes")
                        .default_open(true)
                        .show(ui, |ui| {
                            param_f32(
                                ui,
                                "Inclinaison",
                                "°",
                                &mut self.sculpt_brush.dip_deg,
                                0.0,
                                45.0,
                                0.0,
                            );
                            if self.sculpt_brush.dip_deg > 0.1 {
                                param_f32(
                                    ui,
                                    "Azimut du pendage",
                                    "°",
                                    &mut self.sculpt_brush.dip_azimuth_deg,
                                    0.0,
                                    360.0,
                                    0.0,
                                );
                            }
                            param_f32(
                                ui,
                                "Banc / falaise",
                                "",
                                &mut self.sculpt_brush.bench_ratio,
                                0.3,
                                0.99,
                                0.82,
                            );
                            param_f32(
                                ui,
                                "Variation des bancs",
                                "",
                                &mut self.sculpt_brush.bench_variation,
                                0.0,
                                1.0,
                                0.25,
                            );
                            param_f32(
                                ui,
                                "Netteté des risers",
                                "",
                                &mut self.sculpt_brush.riser_sharpness,
                                0.0,
                                1.0,
                                0.7,
                            );
                            ui.separator();
                            ui.label(
                                egui::RichText::new("Orgues verticales (0 = désactivées)")
                                    .small()
                                    .weak(),
                            );
                            param_f32(
                                ui,
                                "Espacement",
                                "m",
                                &mut self.sculpt_brush.column_spacing_m,
                                0.0,
                                200.0,
                                0.0,
                            );
                            if self.sculpt_brush.column_spacing_m > 0.0 {
                                param_f32(
                                    ui,
                                    "Profondeur",
                                    "m",
                                    &mut self.sculpt_brush.column_depth_m,
                                    0.0,
                                    50.0,
                                    2.0,
                                );
                                param_f32(
                                    ui,
                                    "Irrégularité",
                                    "",
                                    &mut self.sculpt_brush.column_jitter,
                                    0.0,
                                    1.0,
                                    0.35,
                                );
                            }
                        });
                }
                if self.sculpt_brush.mode == BrushMode::Groove {
                    egui::CollapsingHeader::new("Orgues de pente")
                        .default_open(true)
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(
                                    "Des sillons alignés sur la descente, comme les joints d'une coulée basaltique. Intensité = profondeur.",
                                )
                                .small()
                                .weak(),
                            );
                            param_f32(
                                ui,
                                "Espacement",
                                "m",
                                &mut self.sculpt_brush.column_spacing_m,
                                0.0,
                                200.0,
                                0.0,
                            );
                            if self.sculpt_brush.column_spacing_m > 0.0 {
                                param_f32(
                                    ui,
                                    "Irrégularité",
                                    "",
                                    &mut self.sculpt_brush.column_jitter,
                                    0.0,
                                    1.0,
                                    0.35,
                                );
                            }
                        });
                }
                if self.sculpt_brush.mode == BrushMode::Clone {
                    ui.horizontal(|ui| {
                        ui.label("Source");
                        ui.add(
                            egui::DragValue::new(&mut self.sculpt_brush.clone_offset_m[0])
                                .speed(1.0)
                                .prefix("x "),
                        );
                        ui.add(
                            egui::DragValue::new(&mut self.sculpt_brush.clone_offset_m[1])
                                .speed(1.0)
                                .prefix("y "),
                        );
                    });
                }
                ui.checkbox(&mut self.sculpt_brush.invert, "Inverser");
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Sculpt,
        );

        let header = egui::CollapsingHeader::new("Symétrie et presets")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Symétrie");
                    egui::ComboBox::from_id_salt("sculpt-symmetry")
                        .selected_text(symmetry_label(self.sculpt_brush.symmetry))
                        .show_ui(ui, |ui| {
                            for symmetry in [
                                Symmetry::None,
                                Symmetry::X,
                                Symmetry::Y,
                                Symmetry::Xy,
                                Symmetry::Radial(6),
                            ] {
                                ui.selectable_value(
                                    &mut self.sculpt_brush.symmetry,
                                    symmetry,
                                    symmetry_label(symmetry),
                                );
                            }
                        });
                });
                if let Symmetry::Radial(count) = &mut self.sculpt_brush.symmetry {
                    let mut value = *count as f32;
                    if param_f32(ui, "Branches", "", &mut value, 2.0, 24.0, 6.0) {
                        *count = value.round() as u32;
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Roche").clicked() {
                        self.sculpt_brush.mode = BrushMode::Raise;
                        self.sculpt_brush.radius_m = 60.0;
                        self.sculpt_brush.strength = 6.0;
                        self.sculpt_brush.roughness = 0.35;
                        self.sculpt_brush.falloff = Falloff::Smooth;
                    }
                    if ui.button("Détail fin").clicked() {
                        self.sculpt_brush.mode = BrushMode::Noise;
                        self.sculpt_brush.radius_m = 25.0;
                        self.sculpt_brush.strength = 2.0;
                        self.sculpt_brush.roughness = 0.65;
                        self.sculpt_brush.noise_scale_m = 5.0;
                        self.sculpt_brush.aa_samples = 6;
                    }
                    if ui.button("Érosion").clicked() {
                        self.sculpt_brush.mode = BrushMode::Smooth;
                        self.sculpt_brush.radius_m = 120.0;
                        self.sculpt_brush.strength = 0.35;
                        self.sculpt_brush.falloff = Falloff::Gaussian;
                    }
                    if ui.button("Cratère").clicked() {
                        self.sculpt_brush.mode = BrushMode::Crater;
                        self.sculpt_brush.radius_m = 80.0;
                        self.sculpt_brush.strength = 12.0;
                    }
                    if ui
                        .button("Terrasses basaltiques")
                        .on_hover_text(
                            "Bancs fins, risers raides et orgues verticales. Densifiez à ≤ 8 m/cellule pour les bancs serrés.",
                        )
                        .clicked()
                    {
                        self.sculpt_brush.mode = BrushMode::Terrace;
                        self.sculpt_brush.radius_m = 180.0;
                        self.sculpt_brush.strength = 1.0;
                        self.sculpt_brush.hardness = 0.55;
                        self.sculpt_brush.roughness = 0.12;
                        self.sculpt_brush.aa_samples = 4;
                        self.sculpt_brush.falloff = Falloff::Smooth;
                        self.sculpt_brush.step_m = 8.0;
                        self.sculpt_brush.dip_deg = 3.0;
                        self.sculpt_brush.bench_ratio = 0.86;
                        self.sculpt_brush.bench_variation = 0.40;
                        self.sculpt_brush.riser_sharpness = 0.85;
                        self.sculpt_brush.column_spacing_m = 18.0;
                        self.sculpt_brush.column_depth_m = 2.5;
                        self.sculpt_brush.column_jitter = 0.5;
                        self.sculpt_status =
                            "Preset terrasses basaltiques : densifiez à ≤ 8 m/cellule si nécessaire."
                                .into();
                    }
                    if ui
                        .button("Orgues de pente")
                        .on_hover_text(
                            "Sillons verticaux alignés sur la pente, pour une paroi basaltique.",
                        )
                        .clicked()
                    {
                        self.sculpt_brush.mode = BrushMode::Groove;
                        self.sculpt_brush.radius_m = 90.0;
                        self.sculpt_brush.strength = 3.5;
                        self.sculpt_brush.hardness = 0.5;
                        self.sculpt_brush.roughness = 0.15;
                        self.sculpt_brush.aa_samples = 4;
                        self.sculpt_brush.falloff = Falloff::Smooth;
                        self.sculpt_brush.column_spacing_m = 12.0;
                        self.sculpt_brush.column_jitter = 0.4;
                        self.sculpt_status =
                            "Preset orgues : appliquez sur une pente raide, puis recalculer le hillshade."
                                .into();
                    }
                });
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Sculpt,
        );

        let header = egui::CollapsingHeader::new("Historique")
            .default_open(true)
            .show(ui, |ui| {
                let stats = self.history.stats();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(self.history.can_undo(), egui::Button::new("Annuler"))
                        .clicked()
                    {
                        self.undo_history(ctx);
                    }
                    if ui
                        .add_enabled(self.history.can_redo(), egui::Button::new("Rétablir"))
                        .clicked()
                    {
                        self.redo_history(ctx);
                    }
                });
                ui.label(
                    egui::RichText::new(format!(
                        "{} action(s) à annuler · {} à rétablir · {} Mo",
                        stats.undo_steps,
                        stats.redo_steps,
                        stats.bytes / 1_048_576
                    ))
                    .small()
                    .weak(),
                );
                if ui.button("Vider l'historique").clicked() {
                    self.history.clear(&self.project, &self.heightfield);
                }
                ui.label(egui::RichText::new(&self.sculpt_status).small().weak());
                ui.separator();
                if ui.button("Recalculer le hillshade").clicked() {
                    self.sculpt_texture_dirty = true;
                }
                ui.label(
                    egui::RichText::new(
                        "Clic gauche : sculpter. Clic droit ou molette : déplacer. \
                         Molette : zoom. Chaque trait est annulable.",
                    )
                    .small()
                    .weak(),
                );
            });
        self.help.register(
            header.header_response.rect.union(
                header
                    .body_response
                    .as_ref()
                    .map_or(egui::Rect::NOTHING, |body| body.rect),
            ),
            help::HelpTopic::Sculpt,
        );

        let _ = ctx;
    }

    fn sculpt_preview_factor(&self) -> u32 {
        // Allow a sharp 2x preview on a 2050² project (about 16.8 M pixels,
        // 67 MB as RGBA) but stop before 4096² would allocate a gigabyte.
        let base = (self.heightfield.width() * self.heightfield.height()).max(1);
        let cap = (24_000_000 / base).clamp(1, 3) as u32;
        self.sculpt_detail.clamp(1, cap)
    }

    fn ensure_hillshade(&mut self, ctx: &egui::Context) {
        if self.sculpt_texture.is_some() && !self.sculpt_texture_dirty {
            return;
        }
        if self.hillshade_task.is_some() {
            ctx.request_repaint_after(Duration::from_millis(40));
            return;
        }
        let params = HillshadeParams {
            azimuth_deg_from_south: self.project.sun.azimuth_deg_from_south,
            declination_deg: self.project.sun.declination_deg,
            vertical_exaggeration: self.sculpt_exaggeration,
            ambient: 0.22,
        };
        let factor = self.sculpt_preview_factor();
        let contours = self.sculpt_contours_m;
        let heightfield = Arc::clone(&self.heightfield);
        let revision = self.hillshade_revision;
        let (sender, receiver) = mpsc::channel();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let shaded = vf_terrain::hillshade_sculpt(&heightfield, params, factor, contours);
            let _ = sender.send(shaded);
            repaint.request_repaint();
        });
        self.hillshade_task = Some(HillshadeTask { receiver, revision });
        self.sculpt_texture_dirty = false;
        self.sculpt_status = "Calcul de l'ombrage en arrière-plan…".into();
    }

    fn poll_hillshade(&mut self, ctx: &egui::Context) {
        let Some(task) = &self.hillshade_task else {
            return;
        };
        match task.receiver.try_recv() {
            Ok(shaded) => {
                let revision = task.revision;
                self.hillshade_task = None;
                if revision == self.hillshade_revision {
                    let image =
                        egui::ColorImage::from_gray([shaded.width, shaded.height], &shaded.pixels);
                    self.sculpt_texture = Some(ctx.load_texture(
                        "sculpt-hillshade",
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                    self.sculpt_texture_dirty = false;
                    if self.sculpt_status == "Calcul de l'ombrage en arrière-plan…" {
                        self.sculpt_status = "Ombrage à jour.".into();
                    }
                } else {
                    // The terrain or the light changed while the worker was
                    // busy: compute a fresh pass instead of showing stale shade.
                    self.sculpt_texture_dirty = true;
                }
            }
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(40));
            }
            Err(TryRecvError::Disconnected) => {
                self.hillshade_task = None;
                self.sculpt_texture_dirty = true;
            }
        }
    }

    /// Double the working resolution so finer terraces can be carved. The
    /// heightfield is bicubically resampled on a worker thread; the history
    /// keeps the operation undoable.
    fn densify_sculpt_terrain(&mut self, ctx: &egui::Context) {
        let width = self.heightfield.width();
        let height = self.heightfield.height();
        let new_width = (width * 2).min(4096);
        let new_height = (height * 2).min(4096);
        if new_width <= width && new_height <= height {
            self.sculpt_status = "Définition maximale atteinte (4096²).".into();
            return;
        }
        let heightfield = Arc::clone(&self.heightfield);
        let title = format!("Densification {width}² → {new_width}²…");
        if self.start_task(
            ctx,
            jobs::TaskKind::Densify,
            title.clone(),
            move |progress| {
                progress.set_phase("Ré-échantillonnage bicubique du terrain…");
                progress.set_indeterminate();
                match vf_terrain::resample(
                    &heightfield,
                    new_width,
                    new_height,
                    ResampleFilter::Bicubic,
                ) {
                    Ok(heightfield) => {
                        progress.set(1.0);
                        jobs::TaskResult::TerrainDensified {
                            heightfield,
                            old_width: width,
                            new_width,
                        }
                    }
                    Err(error) => {
                        jobs::TaskResult::Failed(format!("Densification impossible : {error}"))
                    }
                }
            },
        ) {
            self.sculpt_status = format!("{title} (l'interface reste utilisable)");
            self.status = self.sculpt_status.clone();
        }
    }

    fn sculpt_canvas_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.ensure_hillshade(ctx);

        let available = ui.available_size();
        let (rect, response) = ui.allocate_exact_size(available, egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(12, 15, 19));

        // Do not call `bounds()` here: it scans the whole heightfield and this
        // canvas is redrawn every frame. Origin and spacing give the exact
        // same rectangle without touching the samples.
        let (origin_x, origin_y) = self.heightfield.origin();
        let (spacing_x, spacing_y) = self.heightfield.spacing();
        let min_x = origin_x;
        let max_y = origin_y + (self.heightfield.height() - 1) as f64 * spacing_y;
        let world_w = ((self.heightfield.width() - 1) as f64 * spacing_x).max(1.0);
        let world_h = ((self.heightfield.height() - 1) as f64 * spacing_y).max(1.0);
        let fit = ((rect.width() as f64 / world_w).min(rect.height() as f64 / world_h) * 0.92)
            as f32
            * self.sculpt_zoom;
        let image_size = egui::vec2(world_w as f32 * fit, world_h as f32 * fit);
        let center = rect.center() + self.sculpt_pan;
        let image_rect = egui::Rect::from_center_size(center, image_size);

        if let Some(texture) = &self.sculpt_texture {
            painter.image(
                texture.id(),
                image_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        painter.rect_stroke(
            image_rect,
            0.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(90, 100, 115)),
            egui::StrokeKind::Inside,
        );

        let to_world = |screen: egui::Pos2| -> (f64, f64) {
            let u = ((screen.x - image_rect.left()) / image_size.x.max(1.0)).clamp(0.0, 1.0);
            let v = ((screen.y - image_rect.top()) / image_size.y.max(1.0)).clamp(0.0, 1.0);
            (
                min_x + f64::from(u) * world_w,
                max_y - f64::from(v) * world_h,
            )
        };
        let to_screen = |world: (f64, f64)| -> egui::Pos2 {
            let u = ((world.0 - min_x) / world_w) as f32;
            let v = ((max_y - world.1) / world_h) as f32;
            egui::pos2(
                image_rect.left() + u * image_size.x,
                image_rect.top() + v * image_size.y,
            )
        };

        if response.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > 0.1 {
                let old_zoom = self.sculpt_zoom;
                self.sculpt_zoom = (self.sculpt_zoom * (scroll * 0.0015).exp()).clamp(0.2, 40.0);
                if let Some(pointer) = response.hover_pos() {
                    let offset = pointer - center;
                    self.sculpt_pan += offset * (1.0 - self.sculpt_zoom / old_zoom);
                }
            }
        }
        if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            self.sculpt_pan += response.drag_delta();
        }

        if let Some(pointer) = response.hover_pos() {
            self.sculpt_cursor = Some(to_world(pointer));
        } else {
            self.sculpt_cursor = None;
        }

        let primary_pressed = ui.input(|input| input.pointer.primary_pressed());
        let primary_released = ui.input(|input| input.pointer.primary_released());
        if primary_pressed
            && self.task.is_none()
            && response.contains_pointer()
            && let Some(pointer) = response.hover_pos()
        {
            let world = to_world(pointer);
            // `terrain_stats_hf` keeps one Arc alive; drop it so the first
            // brush stamp mutates the terrain in place instead of cloning it.
            self.invalidate_terrain_stats();
            self.history.begin_sculpt(&self.project, &self.heightfield);
            let mut stroke = Stroke::begin(
                &self.heightfield,
                self.project.seed.derive("sculpt"),
                "Sculpter",
            );
            stroke.stamp_to(
                Arc::make_mut(&mut self.heightfield),
                &self.sculpt_brush,
                world.0,
                world.1,
            );
            self.sculpt_stroke = Some(stroke);
            self.sculpt_status = format!(
                "Trait en cours · {} · rayon {:.0} m",
                brush_mode_label(self.sculpt_brush.mode),
                self.sculpt_brush.radius_m
            );
        }
        if response.dragged_by(egui::PointerButton::Primary)
            && let Some(pointer) = response.hover_pos()
            && let Some(mut stroke) = self.sculpt_stroke.take()
        {
            let world = to_world(pointer);
            stroke.stamp_to(
                Arc::make_mut(&mut self.heightfield),
                &self.sculpt_brush,
                world.0,
                world.1,
            );
            self.sculpt_stroke = Some(stroke);
        }
        if primary_released && let Some(stroke) = self.sculpt_stroke.take() {
            if let Some(undo) = stroke.finish(&self.heightfield) {
                let bytes = undo.bytes();
                self.project.mark_modified();
                self.history
                    .push_sculpt(undo, &self.project, &self.heightfield);
                self.sculpt_texture_dirty = true;
                self.invalidate_terrain_stats();
                self.mark_dirty();
                self.sculpt_status = format!(
                    "Trait appliqué · {} action(s) · {} Ko",
                    self.history.stats().undo_steps,
                    bytes / 1024
                );
                self.start_render(ctx.clone(), false);
            } else {
                // The stroke did not touch a valid sample: close the gesture
                // so the global history keeps observing later edits.
                self.history.end_sculpt(&self.project, &self.heightfield);
                self.sculpt_status = "Trait hors du terrain : rien à annuler.".into();
            }
        }

        if let Some(world) = self.sculpt_cursor {
            let screen = to_screen(world);
            let radius_px = (f64::from(self.sculpt_brush.radius_m) * f64::from(fit)) as f32;
            let color = egui::Color32::from_rgb(255, 210, 120);
            painter.circle_stroke(screen, radius_px, egui::Stroke::new(1.5, color));
            painter.circle_stroke(
                screen,
                radius_px * 0.5,
                egui::Stroke::new(1.0, color.gamma_multiply(0.35)),
            );
            painter.line_segment(
                [
                    screen - egui::vec2(radius_px + 6.0, 0.0),
                    screen - egui::vec2(radius_px + 2.0, 0.0),
                ],
                egui::Stroke::new(1.0, color),
            );
            painter.line_segment(
                [
                    screen + egui::vec2(radius_px + 2.0, 0.0),
                    screen + egui::vec2(radius_px + 6.0, 0.0),
                ],
                egui::Stroke::new(1.0, color),
            );
            painter.text(
                screen + egui::vec2(radius_px + 8.0, -8.0),
                egui::Align2::LEFT_TOP,
                format!("{:.0} m", self.sculpt_brush.radius_m),
                egui::FontId::monospace(11.0),
                color,
            );
        }

        painter.text(
            rect.left_top() + egui::vec2(10.0, 10.0),
            egui::Align2::LEFT_TOP,
            format!(
                "{}×{} · {} · {:.0} m · zoom {:.1}×",
                self.heightfield.width(),
                self.heightfield.height(),
                brush_mode_label(self.sculpt_brush.mode),
                self.sculpt_brush.radius_m,
                self.sculpt_zoom
            ),
            egui::FontId::proportional(13.0),
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 190),
        );
    }

    fn leave_animation_shot_for_orbit(&mut self) {
        if self.workspace == Workspace::Animation
            && self.anim_view == animation_ui::AnimViewMode::Camera
        {
            self.anim_playing = false;
            self.anim_view = animation_ui::AnimViewMode::Design;
            self.anim_design_camera = None;
            self.sync_orbit_from_camera();
        }
    }

    fn viewport_ui(&mut self, ui: &mut egui::Ui) {
        let gpu_active = self.gpu_active();
        let texture = self.texture.clone();
        let ppp = ui.ctx().pixels_per_point();
        let (rect, response, image_px) = if gpu_active {
            // The live GPU view uses the whole central panel.
            let rect = ui.available_rect_before_wrap();
            let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
            let pixels = [
                (rect.width() * ppp).round().max(1.0) as u32,
                (rect.height() * ppp).round().max(1.0) as u32,
            ];
            (rect, response, pixels)
        } else if let Some(texture) = texture {
            let available = ui.available_size();
            let image_px = [texture.size()[0] as u32, texture.size()[1] as u32];
            let image_size = egui::vec2(image_px[0] as f32, image_px[1] as f32);
            let scale = (available.x / image_size.x)
                .min(available.y / image_size.y)
                .max(0.01);
            let size = image_size * scale;
            let (rect, response) = ui
                .with_layout(
                    egui::Layout::centered_and_justified(egui::Direction::TopDown),
                    |ui| ui.allocate_exact_size(size, egui::Sense::click_and_drag()),
                )
                .inner;
            ui.painter().image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
            (rect, response, image_px)
        } else {
            ui.with_layout(
                egui::Layout::centered_and_justified(egui::Direction::TopDown),
                |ui| {
                    ui.horizontal(|ui| {
                        if self.render_task.is_some() {
                            ui.spinner();
                        }
                        ui.label("Preparing preview…");
                    });
                },
            );
            return;
        };

        self.last_viewport_rect = rect;

        // Before/after split. The CPU preview is drawn full-size first; the
        // frozen image is then repainted over the left part with a clip rect,
        // and a draggable vertical line marks the boundary.
        if self.compare.enabled {
            let line_x = rect.left() + rect.width() * self.compare.split.clamp(0.0, 1.0);
            let pointer = ui.input(|input| input.pointer.interact_pos());
            if let Some(pointer) = pointer
                && response.contains_pointer()
                && ui.input(|input| input.pointer.primary_pressed())
                && (pointer.x - line_x).abs() <= 16.0
            {
                self.compare.dragging = true;
            }
            if !ui.input(|input| input.pointer.primary_down()) {
                self.compare.dragging = false;
            }
            if self.compare.dragging
                && let Some(pointer) = pointer
            {
                self.compare.split =
                    ((pointer.x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0);
            }
            let line_x = rect.left() + rect.width() * self.compare.split.clamp(0.0, 1.0);
            if let Some(before) = self.compare.before.clone() {
                // Draw the frozen image first, then the live image clipped to
                // the right. This makes the left/right ownership explicit and
                // avoids relying on the texture painted earlier by the
                // viewport path.
                ui.painter().image(
                    before.id(),
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                if let Some(after) = self.texture.clone() {
                    let clip = egui::Rect::from_min_max(egui::pos2(line_x, rect.top()), rect.max);
                    ui.painter().with_clip_rect(clip).image(
                        after.id(),
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
            }
            let stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 235, 180));
            ui.painter().line_segment(
                [
                    egui::pos2(line_x, rect.top()),
                    egui::pos2(line_x, rect.bottom()),
                ],
                stroke,
            );
            ui.painter().circle_filled(
                egui::pos2(line_x, rect.center().y),
                7.0,
                egui::Color32::from_rgb(255, 235, 180),
            );
            ui.painter().circle_stroke(
                egui::pos2(line_x, rect.center().y),
                8.0,
                egui::Stroke::new(1.0, egui::Color32::BLACK),
            );
            ui.painter().text(
                rect.left_top() + egui::vec2(10.0, 30.0),
                egui::Align2::LEFT_TOP,
                "AVANT",
                egui::FontId::proportional(13.0),
                egui::Color32::from_rgb(255, 235, 180),
            );
            ui.painter().text(
                rect.right_top() + egui::vec2(-10.0, 30.0),
                egui::Align2::RIGHT_TOP,
                "APRÈS",
                egui::FontId::proportional(13.0),
                egui::Color32::from_rgb(255, 235, 180),
            );
            let button_size = egui::vec2(92.0, 24.0);
            let apply_rect = egui::Rect::from_min_size(
                egui::pos2(rect.left() + 10.0, rect.bottom() - 34.0),
                button_size,
            );
            let revert_rect = egui::Rect::from_min_size(
                egui::pos2(rect.left() + 108.0, rect.bottom() - 34.0),
                button_size,
            );
            let apply = ui.put(apply_rect, egui::Button::new("Appliquer"));
            let revert = ui.put(revert_rect, egui::Button::new("Revenir"));
            if apply.clicked() {
                self.compare_apply(ui.ctx());
            } else if revert.clicked() {
                self.compare_revert(ui.ctx());
            }
        }

        if gpu_active && let Some(gpu) = self.gpu.as_ref() {
            let camera = self.project.camera.clone();
            if let Some(callback) = gpu.callback(&camera, &self.project, image_px[0], image_px[1]) {
                ui.painter()
                    .add(eframe::egui_wgpu::Callback::new_paint_callback(
                        rect, callback,
                    ));
            }
        }

        let mut editor_consumed = false;
        let mut pointer_over_transport = false;
        if self.workspace == Workspace::Animation
            && self.anim_view == animation_ui::AnimViewMode::Design
        {
            let camera = self.project.camera.clone();
            let trail = if self.anim_editor.show_trail {
                let stale = self
                    .anim_trail_cache
                    .as_ref()
                    .is_none_or(|(revision, _)| *revision != self.anim_revision);
                if stale && !self.anim_editor.is_dragging() {
                    let computed = animation_ui::trail(
                        &self.project.rig,
                        &self.heightfield,
                        self.project.seed,
                    );
                    self.anim_trail_cache = Some((self.anim_revision, computed));
                }
                self.anim_trail_cache
                    .as_ref()
                    .map(|(_, trail)| trail.clone())
            } else {
                None
            };
            let comparison = if self.anim_compare {
                let stale = self
                    .anim_compare_cache
                    .as_ref()
                    .is_none_or(|(revision, _)| *revision != self.anim_revision);
                if stale && !self.anim_editor.is_dragging() {
                    let computed = animation_ui::comparison_trails(
                        &self.project.rig,
                        &self.heightfield,
                        self.project.seed,
                    );
                    self.anim_compare_cache = Some((self.anim_revision, computed));
                }
                self.anim_compare_cache
                    .as_ref()
                    .map(|(_, trails)| trails.clone())
            } else {
                None
            };
            let blocked = self.help.last_rect().union(self.anim_transport_rect);
            let editor = animation_editor::show(
                ui,
                rect,
                &camera,
                &mut self.project.rig,
                &self.heightfield,
                self.rig_time,
                &mut self.anim_editor,
                trail.as_deref(),
                comparison.as_deref(),
                self.anim_live_sample.as_ref(),
                blocked,
            );
            editor_consumed = editor.consumed;
            if editor.changed {
                self.anim_revision = self.anim_revision.wrapping_add(1);
                self.anim_profile_cache = None;
                self.anim_compare_cache = None;
                self.anim_camera_dirty = true;
                self.project.mark_modified();
            }
            if let Some(time) = editor.last_added_time {
                self.rig_time = time;
            }
            if let Some(status) = editor.status {
                self.rig_status = status;
            }
            match self.anim_editor.selection {
                Some(animation_ui::AnimSelection::TargetKey(_)) => self.anim_edit_target = true,
                Some(
                    animation_ui::AnimSelection::EyeKey(_)
                    | animation_ui::AnimSelection::FixedTarget,
                ) => {
                    self.anim_edit_target = false;
                }
                None => {}
            }
            if editor.changed {
                ui.ctx().request_repaint();
            }
        }
        if self.workspace == Workspace::Animation {
            pointer_over_transport = self.anim_transport_rect.contains(
                ui.input(|input| input.pointer.interact_pos())
                    .unwrap_or(egui::Pos2::ZERO),
            );
            self.animation_transport_ui(ui.ctx(), rect);
            if self.anim_view == animation_ui::AnimViewMode::Design && self.anim_compare {
                draw_animation_legend(ui, rect, self.project.rig.body.preset);
            }
        }

        let gizmo_rect = if self.show_gizmo {
            view_gizmo::rect_for(rect)
        } else {
            None
        };
        let pointer_over_gizmo = gizmo_rect.is_some_and(|gizmo| {
            ui.input(|input| {
                input
                    .pointer
                    .interact_pos()
                    .is_some_and(|pointer| gizmo.contains(pointer))
            })
        });
        let shift_held = ui.input(|input| input.modifiers.shift);
        let pan_drag = response.dragged_by(egui::PointerButton::Middle)
            || response.dragged_by(egui::PointerButton::Secondary)
            || (response.dragged_by(egui::PointerButton::Primary) && shift_held);
        if pan_drag
            && !pointer_over_gizmo
            && !editor_consumed
            && !pointer_over_transport
            && !self.compare.enabled
        {
            // Pan in the screen plane: move the orbit pivot (and therefore the
            // camera) along the current camera right/up axes. The previous
            // controls could only orbit and zoom, which made it impossible to
            // frame a ridge without changing the viewing angle.
            self.leave_animation_shot_for_orbit();
            let delta = response.drag_delta();
            if delta.length_sq() > 0.0 {
                let camera = &self.project.camera;
                let right = camera.right();
                let up = camera.true_up();
                let world_per_px =
                    self.distance * 2.0 * (f64::from(camera.fov_y_deg).to_radians() * 0.5).tan()
                        / f64::from(rect.height().max(1.0));
                let world = right * (-f64::from(delta.x) * world_per_px)
                    + up * (f64::from(delta.y) * world_per_px);
                self.orbit_center += world;
                self.apply_orbit();
                self.mark_camera_dirty();
            }
        } else if response.dragged()
            && !pointer_over_gizmo
            && !editor_consumed
            && !pointer_over_transport
            && !self.compare.enabled
            && !shift_held
        {
            self.leave_animation_shot_for_orbit();
            let delta = response.drag_delta();
            self.yaw = (self.yaw + delta.x * 0.25).clamp(-180.0, 180.0);
            self.pitch = (self.pitch - delta.y * 0.25).clamp(-90.0, 90.0);
            self.apply_orbit();
            self.mark_camera_dirty();
        }
        if response.double_clicked()
            && !pointer_over_gizmo
            && !editor_consumed
            && !pointer_over_transport
        {
            self.home_view();
        }
        if response.hovered() && !pointer_over_gizmo && !editor_consumed && !pointer_over_transport
        {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > 0.1 {
                self.leave_animation_shot_for_orbit();
                self.distance =
                    (self.distance * (-f64::from(scroll) * 0.0015).exp()).clamp(100.0, 80_000.0);
                self.apply_orbit();
                self.mark_camera_dirty();
            }
        }

        if self.show_gizmo {
            let camera = self.project.camera.clone();
            let gizmo = view_gizmo::show(ui, rect, &camera);
            // The viewport owns the whole image and may win the click
            // hit-test; fall back to the gizmo's hover target in that case.
            let action = gizmo.action.or_else(|| {
                if response.clicked() && pointer_over_gizmo {
                    gizmo.hovered
                } else {
                    None
                }
            });
            if let Some(action) = action {
                self.apply_gizmo_action(action);
                ui.ctx().request_repaint();
            }
        }

        let view_name = vf_scene::ViewPreset::from_normal(-self.project.camera.forward())
            .map_or("Perspective", vf_scene::ViewPreset::label);
        let source = if gpu_active {
            self.gpu
                .as_ref()
                .map_or_else(|| "GPU".to_string(), |gpu| format!("GPU · {}", gpu.label()))
        } else {
            format!("aperçu {}×{}", image_px[0], image_px[1])
        };
        let animation = if self.workspace == Workspace::Animation {
            let mode = match self.anim_view {
                animation_ui::AnimViewMode::Design => "design",
                animation_ui::AnimViewMode::Camera => "caméra",
            };
            format!(
                " · Animation {mode} · t={:.2}/{:.2}s{}",
                self.rig_time,
                self.project.rig.duration_s,
                if self.anim_playing { " · lecture" } else { "" }
            )
        } else {
            String::new()
        };
        ui.painter().text(
            rect.left_top() + egui::vec2(10.0, 10.0),
            egui::Align2::LEFT_TOP,
            format!(
                "{} · {} · {}{} · glisser = orbite · Maj/clic droit/molette = pan · molette = zoom · double-clic = défaut",
                self.project.meta.name, view_name, source, animation
            ),
            egui::FontId::proportional(13.0),
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 170),
        );
    }

    fn busy(&self) -> bool {
        self.task.is_some()
            || self.anim_export_task.is_some()
            // A quick adaptive preview is shorter than a frame budget; only
            // give it a gauge once it is actually noticeable, otherwise the
            // bottom panel would flash on every mouse move.
            || self.render_task.as_ref().is_some_and(|task| {
                task.full || task.started.elapsed() >= Duration::from_millis(350)
            })
    }

    /// One persistent gauge for every long-running operation. It is a normal
    /// bottom panel, never a modal dialog: the window keeps pumping messages
    /// and the user can continue to orbit, read help or adjust the scene.
    fn progress_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(task) = &self.task {
                ui.label(egui::RichText::new(task.title.clone()).strong());
                let fraction = task.progress.display_fraction();
                let known = task.progress.fraction().is_some();
                let bar = egui::ProgressBar::new(fraction)
                    .desired_width(240.0)
                    .animate(!known);
                if known {
                    ui.add(bar.show_percentage());
                } else {
                    ui.add(bar.text("en cours"));
                }
                ui.label(egui::RichText::new(task.progress.phase()).small());
                ui.label(
                    egui::RichText::new(format!("{:.1} s", task.elapsed().as_secs_f32()))
                        .small()
                        .weak(),
                );
            } else if let Some(task) = &self.anim_export_task {
                let fraction = if task.total == 0 {
                    0.0
                } else {
                    task.done as f32 / task.total as f32
                };
                ui.label(egui::RichText::new("Rendu de la séquence d'animation").strong());
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_width(240.0)
                        .text(format!("{}/{} images", task.done, task.total)),
                );
                ui.label(
                    egui::RichText::new(format!("{:.1} s", task.started.elapsed().as_secs_f32()))
                        .small()
                        .weak(),
                );
                if ui.button("Annuler").clicked() {
                    task.cancel.store(true, Ordering::Relaxed);
                }
            } else if let Some(task) = &self.render_task {
                let elapsed = task.started.elapsed().as_secs_f32();
                let expected = (self.last_render_ms as f32 / 1000.0).max(0.6);
                let fraction = (elapsed / expected).clamp(0.02, 0.95);
                ui.label(
                    egui::RichText::new(if task.full {
                        "Rendu final CPU"
                    } else {
                        "Aperçu CPU"
                    })
                    .strong(),
                );
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_width(240.0)
                        .animate(elapsed > expected),
                );
                ui.label(
                    egui::RichText::new(format!("{elapsed:.1} s · la scène reste manipulable"))
                        .small()
                        .weak(),
                );
            }
        });
    }

    /// Preferences window: help behaviour and configurable undo/redo limits.
    fn preferences_window(&mut self, ctx: &egui::Context) {
        if !self.preferences_open {
            return;
        }
        let mut open = self.preferences_open;
        let mut help_enabled = self.help.enabled;
        let mut history_settings = self.history.settings().clone();
        let stats = self.history.stats();
        let mut clear_history = false;
        let mut reset_history = false;
        let mut open_help_workspace = false;
        egui::Window::new("Préférences")
            .open(&mut open)
            .resizable(true)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .default_width(430.0)
            .show(ctx, |ui| {
                ui.heading("Aide");
                ui.label(
                    egui::RichText::new(
                        "L'aide contextuelle est désactivée au démarrage. Une fois activée, la carte reste ouverte sur le dernier réglage survolé : vous pouvez la déplacer, la faire défiler et cliquer dans son sommaire. L'espace Aide reste toujours accessible sans l'activer.",
                    )
                    .small(),
                );
                ui.checkbox(&mut help_enabled, "Activer l'aide flottante");
                if ui.button("Ouvrir la rubrique Aide").clicked() {
                    open_help_workspace = true;
                }
                ui.add_space(6.0);
                ui.separator();
                ui.heading("Historique");
                ui.label(
                    egui::RichText::new(
                        "Chaque geste (un glisser de curseur, une génération, un trait de pinceau…) devient une étape une fois la souris au repos. Les mouvements de caméra ne sont pas enregistrés.",
                    )
                    .small(),
                );
                ui.checkbox(&mut history_settings.enabled, "Activer Annuler/Rétablir");
                ui.add_enabled_ui(history_settings.enabled, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Profondeur");
                        ui.add(
                            egui::Slider::new(&mut history_settings.max_steps, 1..=200)
                                .text("étapes"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Mémoire max");
                        ui.add(
                            egui::Slider::new(&mut history_settings.memory_limit_mb, 16..=8192)
                                .logarithmic(true)
                                .text("Mo"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Regrouper après");
                        ui.add(
                            egui::Slider::new(&mut history_settings.merge_delay_ms, 0..=2000)
                                .text("ms"),
                        );
                    });
                    ui.checkbox(
                        &mut history_settings.include_sculpt,
                        "Historiser chaque trait de pinceau",
                    );
                });
                ui.label(
                    egui::RichText::new(format!(
                        "{} étape(s) à annuler · {} à rétablir · {} / {} Mo",
                        stats.undo_steps,
                        stats.redo_steps,
                        stats.bytes / 1_048_576,
                        stats.limit_bytes / 1_048_576
                    ))
                    .small()
                    .weak(),
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            stats.undo_steps + stats.redo_steps > 0,
                            egui::Button::new("Vider l'historique"),
                        )
                        .clicked()
                    {
                        clear_history = true;
                    }
                    if ui
                        .button("Réinitialiser les réglages")
                        .on_hover_text("Profondeur 30, mémoire 512 Mo, regroupement 400 ms")
                        .clicked()
                    {
                        reset_history = true;
                    }
                });
                ui.separator();
                ui.label(
                    egui::RichText::new(
                        "Raccourcis : Ctrl+Z annuler · Ctrl+Y ou Ctrl+Maj+Z rétablir · F1 aide flottante · Échap ferme la carte.",
                    )
                    .small()
                    .weak(),
                );
            });
        self.preferences_open = open;
        if help_enabled != self.help.enabled {
            if help_enabled {
                self.help.enabled = true;
            } else {
                self.help.close();
            }
        }
        if open_help_workspace {
            self.workspace = Workspace::Help;
        }
        if reset_history {
            history_settings = history::HistorySettings::default();
        }
        if clear_history {
            self.history.clear(&self.project, &self.heightfield);
        }
        if history_settings != *self.history.settings() {
            let persisted = history_settings.clone();
            self.history
                .set_settings(history_settings, &self.project, &self.heightfield);
            history::save_settings(&persisted);
        }
    }
}

impl eframe::App for VistaForgeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.help.begin_frame();
        // Commit the previous interaction once it has settled. A slider drag
        // is therefore a single undo step, not one step per mouse move.
        self.history.tick(&self.project, &self.heightfield);
        self.poll_task(&ctx);
        self.poll_hillshade(&ctx);
        self.poll_gpu_mesh(&ctx);
        self.poll_render(&ctx);
        self.poll_animation_export(&ctx);
        if let Some(pick) = self.file_picker.show(&ctx) {
            self.apply_file_pick(pick);
        }
        self.pump_gpu(&ctx);
        self.pump_animation(&ctx);
        self.view_shortcuts(&ctx);

        self.pump_preview(&ctx);

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("VistaForge");
                ui.add_space(8.0);
                ui.separator();
                // The project path and Open/Save now live only in the Fichiers
                // workspace: the toolbar keeps a read-only reminder of the
                // current document instead of duplicating the file menu.
                let project_name = self.project_path.as_ref().map_or_else(
                    || "Projet non enregistré".to_string(),
                    |path| {
                        path.file_name().map_or_else(
                            || path.display().to_string(),
                            |name| name.to_string_lossy().into_owned(),
                        )
                    },
                );
                let project_label = ui
                    .label(egui::RichText::new(project_name).small().weak())
                    .on_hover_text(self.project_path.as_ref().map_or_else(
                        || "Utilisez l'espace Fichiers pour choisir un chemin".to_string(),
                        |path| path.display().to_string(),
                    ));
                if project_label.clicked() {
                    self.workspace = Workspace::Files;
                }
                ui.separator();
                let undo = ui
                    .add_enabled(self.history.can_undo(), egui::Button::new("Annuler"))
                    .on_hover_text(self.history.undo_label().map_or_else(
                        || "Annuler (Ctrl+Z)".to_string(),
                        |label| format!("Annuler « {label} » (Ctrl+Z)"),
                    ));
                if undo.clicked() {
                    self.undo_history(&ctx);
                }
                let redo = ui
                    .add_enabled(self.history.can_redo(), egui::Button::new("Rétablir"))
                    .on_hover_text(self.history.redo_label().map_or_else(
                        || "Rétablir (Ctrl+Y)".to_string(),
                        |label| format!("Rétablir « {label} » (Ctrl+Y)"),
                    ));
                if redo.clicked() {
                    self.redo_history(&ctx);
                }
                ui.separator();
                let render_button = ui.button("Render");
                if render_button.hovered() {
                    self.help
                        .register(render_button.rect, help::HelpTopic::RenderFinal);
                }
                if render_button.clicked() {
                    self.start_render(ctx.clone(), true);
                }
                if ui
                    .selectable_label(self.cpu_preview_forced, "Vue CPU")
                    .on_hover_text(
                        "Affiche le rendu CPU complet (détail continu, matières, bump, déplacement, textures importées). Plus lent que la vue GPU mais c'est lui qui montre les effets.",
                    )
                    .clicked()
                {
                    self.cpu_preview_forced = !self.cpu_preview_forced;
                    if self.cpu_preview_forced {
                        self.texture = None;
                        self.mark_dirty();
                        self.start_render(ctx.clone(), false);
                    } else {
                        self.refresh_preview(ctx.clone());
                    }
                }
                let compare_button = ui
                    .selectable_label(self.compare.enabled, "Avant/Après")
                    .on_hover_text(
                        "Compare le rendu CPU figé à l'état courant. Faites glisser la ligne verticale ; Appliquer garde le résultat, Revenir restaure l'état d'avant.",
                    );
                if compare_button.clicked() {
                    if self.compare.enabled {
                        self.compare_apply(&ctx);
                    } else {
                        self.compare_start(&ctx);
                    }
                }
                if self.render_task.is_some() {
                    ui.spinner();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .button("Préférences")
                        .on_hover_text("Aide, historique, limites de mémoire")
                        .clicked()
                    {
                        self.preferences_open = true;
                    }
                    if ui
                        .selectable_label(self.help.enabled, "Aide flottante")
                        .on_hover_text(
                            "Aide contextuelle (F1) : la carte reste ouverte pour être lue",
                        )
                        .clicked()
                    {
                        self.help.toggle(help_topic_for_workspace(self.workspace));
                    }
                    if ui
                        .button("Aide")
                        .on_hover_text("Ouvrir la rubrique d'aide complète")
                        .clicked()
                    {
                        self.workspace = Workspace::Help;
                    }
                    ui.label(egui::RichText::new(&self.status).small());
                });
            });
        });

        egui::Panel::top("functions").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for workspace in Workspace::ALL {
                    let active = self.workspace == workspace;
                    let text = egui::RichText::new(workspace.label()).size(13.0);
                    let tab = ui.selectable_label(active, text);
                    if tab.hovered() {
                        self.help
                            .register(tab.rect, help_topic_for_workspace(workspace));
                    }
                    if tab.clicked() {
                        if self.workspace == Workspace::Animation
                            && workspace != Workspace::Animation
                        {
                            self.anim_playing = false;
                            self.anim_editor.trace = false;
                        }
                        self.workspace = workspace;
                        if workspace == Workspace::Animation {
                            self.ensure_animation_workspace(&ctx);
                        }
                    }
                }
            });
        });

        egui::Panel::left("controls")
            .resizable(true)
            .default_size(360.0)
            .min_size(320.0)
            .max_size(520.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.heading(egui::RichText::new(self.workspace.label()).size(16.0));
                ui.separator();
                self.workspace_panel(ui, &ctx);
            });

        if self.workspace == Workspace::Animation && self.anim_timeline.open {
            let dock_height = if self.anim_timeline.mode == timeline::Mode::Graph {
                260.0
            } else {
                205.0
            };
            egui::Panel::bottom("anim-timeline")
                .resizable(false)
                .exact_size(dock_height)
                .show(ui, |ui| {
                    self.animation_timeline_ui(ui);
                });
        }

        if self.busy() {
            egui::Panel::bottom("progress")
                .resizable(false)
                .exact_size(34.0)
                .show(ui, |ui| {
                    self.progress_panel(ui);
                });
        }

        // Register the overlays that are drawn inside the viewport. Their
        // rectangles are stored from the previous frame because the actual
        // transport/gizmo widgets are created after the help resolution (the
        // help card must be known before the viewport draws).
        if self.workspace == Workspace::Animation && self.anim_transport_rect.is_positive() {
            self.help
                .register(self.anim_transport_rect, help::HelpTopic::Transport);
        }
        if self.show_gizmo
            && self.last_viewport_rect.is_positive()
            && let Some(rect) = view_gizmo::rect_for(self.last_viewport_rect)
        {
            self.help.register(rect, help::HelpTopic::Navigation);
        }
        // Resolve the hovered topic and draw the help card *before* the
        // viewport so the path editor sees its current rectangle and never
        // treats a click in the card as a terrain click.
        self.help.resolve(&ctx);
        let help_viewport = if self.last_viewport_rect.is_positive() {
            self.last_viewport_rect
        } else {
            ctx.content_rect()
        };
        let help_context = help::HelpContext {
            rig: &self.project.rig,
            playing: self.anim_playing,
            time_s: self.rig_time,
            view_mode: match self.anim_view {
                animation_ui::AnimViewMode::Design => "design",
                animation_ui::AnimViewMode::Camera => "caméra",
            },
        };
        if self.workspace != Workspace::Help {
            self.help.show(
                &ctx,
                help_viewport,
                &help_context,
                &help::HelpTopic::ALL,
                if self.workspace == Workspace::Animation {
                    (self.anim_transport_rect.height() + 16.0).max(78.0)
                } else {
                    0.0
                },
            );
        }
        // "Aide" in the floating card opens the dedicated workspace on the
        // same topic, where the full article and a searchable index live.
        if let Some(topic) = self.help.take_browse_request() {
            self.help.set_browse_topic(topic);
            self.workspace = Workspace::Help;
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(egui::Color32::from_rgb(16, 18, 22)))
            .show(ui, |ui| {
                if self.workspace == Workspace::Help {
                    let context = help::HelpContext {
                        rig: &self.project.rig,
                        playing: self.anim_playing,
                        time_s: self.rig_time,
                        view_mode: match self.anim_view {
                            animation_ui::AnimViewMode::Design => "design",
                            animation_ui::AnimViewMode::Camera => "caméra",
                        },
                    };
                    self.help.browse_page_ui(ui, &ctx, &context);
                } else if self.workspace == Workspace::Sculpt {
                    self.sculpt_canvas_ui(ui, &ctx);
                } else {
                    self.viewport_ui(ui);
                }
            });

        self.preferences_window(&ctx);
        self.pump_capture(&ctx);
        // Detect edits made during this pass (widgets mutate the project and
        // the heightfield directly), then commit them after the idle delay.
        self.history.tick(&self.project, &self.heightfield);
    }
}

/// One settings row: label, slider, numeric entry. Double-click the numeric
/// field to restore the default. Used by every workspace panel so the layout
/// stays consistent.
fn render_mode_label(mode: RenderMode) -> &'static str {
    mode.label()
}

fn brush_mode_label(mode: BrushMode) -> &'static str {
    match mode {
        BrushMode::Raise => "Élever",
        BrushMode::Lower => "Abaisser",
        BrushMode::Smooth => "Lisser",
        BrushMode::Flatten => "Aplatir",
        BrushMode::Terrace => "Terrasse",
        BrushMode::Ridge => "Crête",
        BrushMode::Groove => "Sillon",
        BrushMode::Crater => "Cratère",
        BrushMode::Noise => "Bruit fractal",
        BrushMode::Warp => "Warp",
        BrushMode::Clone => "Cloner",
    }
}

fn brush_shape_label(shape: BrushShape) -> &'static str {
    match shape {
        BrushShape::Circle => "Disque",
        BrushShape::Square => "Carré",
        BrushShape::Diamond => "Losange",
        BrushShape::Ring => "Anneau",
        BrushShape::Star => "Étoile",
        BrushShape::Polygon => "Polygone",
    }
}

fn falloff_label(falloff: Falloff) -> &'static str {
    match falloff {
        Falloff::Constant => "Constant",
        Falloff::Linear => "Linéaire",
        Falloff::Smooth => "Lisse",
        Falloff::Gaussian => "Gaussien",
        Falloff::Plateau => "Plateau",
    }
}

fn symmetry_label(symmetry: Symmetry) -> &'static str {
    match symmetry {
        Symmetry::None => "Aucune",
        Symmetry::X => "Miroir X",
        Symmetry::Y => "Miroir Y",
        Symmetry::Xy => "Miroirs XY",
        Symmetry::Radial(_) => "Radial",
    }
}

/// Duration widget with a logarithmic slider, a fine numeric field and a
/// one-click reset. A linear 0.1–600 s slider made short clips impossible to
/// fine-tune; the log scale gives the same precision at 2 s and at 200 s.
fn duration_param(ui: &mut egui::Ui, rig: &mut CameraRig, stretch: bool) -> bool {
    let mut value = rig.duration_s;
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized(
            [128.0, 18.0],
            egui::Label::new(egui::RichText::new("Durée").size(12.5)).truncate(),
        );
        let width = (ui.available_width() - 96.0).max(60.0);
        changed |= ui
            .add_sized(
                [width, 18.0],
                egui::Slider::new(&mut value, 0.1..=600.0)
                    .logarithmic(true)
                    .show_value(false),
            )
            .changed();
        let response = ui.add_sized(
            [76.0, 18.0],
            egui::DragValue::new(&mut value)
                .speed(0.05)
                .range(0.1..=600.0)
                .max_decimals(2)
                .suffix(" s"),
        );
        changed |= response.changed();
        if response.double_clicked() {
            value = 8.0;
            changed = true;
        }
    });
    if changed {
        if stretch {
            rig.stretch_duration(value);
        } else {
            rig.duration_s = value.clamp(0.1, 600.0);
        }
    }
    changed
}

fn safety_label(report: &vf_flight::safety::SafetyReport) -> String {
    let mut parts = Vec::new();
    if report.ground_active {
        parts.push("garde sol");
    }
    if report.wall_climb {
        parts.push("montée paroi");
    }
    if report.wall_active {
        parts.push(if report.wall_side < 0 {
            "détour paroi G"
        } else {
            "détour paroi D"
        });
    }
    if report.blocked {
        parts.push("paroi bloquée : montée de secours");
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" · {}", parts.join(" · "))
    }
}

fn param_f32(
    ui: &mut egui::Ui,
    label: &str,
    unit: &str,
    value: &mut f32,
    min: f32,
    max: f32,
    default: f32,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized(
            [128.0, 18.0],
            egui::Label::new(egui::RichText::new(label).size(12.5)).truncate(),
        );
        let width = (ui.available_width() - 84.0).max(60.0);
        changed |= ui
            .add_sized(
                [width, 18.0],
                egui::Slider::new(value, min..=max).show_value(false),
            )
            .changed();
        let mut drag = egui::DragValue::new(value)
            .speed(((max - min) / 200.0).max(0.0001))
            .range(min..=max);
        if !unit.is_empty() {
            drag = drag.suffix(format!(" {unit}"));
        }
        let response = ui.add_sized([76.0, 18.0], drag);
        changed |= response.changed();
        if response.double_clicked() {
            *value = default;
            changed = true;
        }
    });
    changed
}

fn param_u32(
    ui: &mut egui::Ui,
    label: &str,
    unit: &str,
    value: &mut u32,
    min: u32,
    max: u32,
    default: u32,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized(
            [128.0, 18.0],
            egui::Label::new(egui::RichText::new(label).size(12.5)).truncate(),
        );
        let width = (ui.available_width() - 84.0).max(60.0);
        changed |= ui
            .add_sized(
                [width, 18.0],
                egui::Slider::new(value, min..=max).show_value(false),
            )
            .changed();
        let mut drag = egui::DragValue::new(value)
            .speed(((max - min) as f64 / 200.0).max(0.1))
            .range(min..=max);
        if !unit.is_empty() {
            drag = drag.suffix(format!(" {unit}"));
        }
        let response = ui.add_sized([76.0, 18.0], drag);
        changed |= response.changed();
        if response.double_clicked() {
            *value = default;
            changed = true;
        }
    });
    changed
}

fn normalized_project_path(raw: &str) -> PathBuf {
    let mut path = PathBuf::from(raw.trim());
    if !path.as_os_str().is_empty() && path.extension().is_none() {
        path.set_extension("vfproj");
    }
    path
}

fn default_scene() -> (Project, HeightField) {
    let seed = Seed::new(42);
    let params = vf_core::terrain_params::modern_preset("alpine")
        .expect("built-in preset")
        .with_size_seed(514, seed.0);
    let mut project = Project::new("Alpine valley", seed, 514);
    project.source = TerrainSource::Modern { params };
    project.geology = vf_core::GeologySettings::preset("sandstone").unwrap_or_default();
    project.erosion_enabled = true;
    project.erosion = ErosionParams::scaled_for(514);
    let heightfield = vf_geology::build(
        &project.source,
        Some(&project.erosion),
        &project.geology,
        seed,
    )
    .expect("default terrain");
    project.sync_terrain(&heightfield);
    project.camera = vf_scene::auto_frame(&heightfield, 16.0 / 9.0);
    project.render.width = 1280;
    project.render.height = 720;
    (project, heightfield)
}

/// Pick the next interactive preview scale from a measured render time.
///
/// Render cost grows with the pixel count, hence the square root. The result
/// is smoothed so a single slow frame (a compile, a disk flush) does not make
/// the preview permanently tiny.
fn adapted_scale(current: f32, measured_scale: f32, elapsed_ms: u128) -> f32 {
    if elapsed_ms == 0 {
        return current;
    }
    let elapsed = elapsed_ms as f32;
    let ideal = (measured_scale * (PREVIEW_TARGET_MS / elapsed).sqrt())
        .clamp(MIN_PREVIEW_SCALE, MAX_INTERACTIVE_SCALE);
    (current * 0.55 + ideal * 0.45).clamp(MIN_PREVIEW_SCALE, MAX_INTERACTIVE_SCALE)
}

/// Next progressive refinement step after a quick interactive pass.
fn next_preview_scale(current: f32) -> f32 {
    let next = (current * 1.5).min(1.0);
    if next - current < 0.08 { 1.0 } else { next }
}

/// Small colour key for the simulated profile trajectories.
fn draw_animation_legend(ui: &mut egui::Ui, rect: egui::Rect, active_preset: AircraftPreset) {
    let painter = ui.painter().with_clip_rect(rect);
    let mut x = rect.left() + 14.0;
    let y = rect.top() + 34.0;
    painter.text(
        egui::Pos2::new(x, y),
        egui::Align2::LEFT_TOP,
        "Trajectoires simulées :",
        egui::FontId::proportional(10.0),
        egui::Color32::from_rgb(185, 198, 214),
    );
    x += 122.0;
    for preset in AircraftPreset::ALL {
        if preset == AircraftPreset::Manual {
            continue;
        }
        let active = preset == active_preset;
        let color = if active {
            egui::Color32::from_rgb(255, 226, 120)
        } else {
            animation_ui::preset_color(preset)
        };
        painter.circle_filled(egui::Pos2::new(x + 4.0, y + 6.0), 3.5, color);
        painter.text(
            egui::Pos2::new(x + 12.0, y),
            egui::Align2::LEFT_TOP,
            if active {
                format!("{} (actif)", preset.label())
            } else {
                preset.label().to_string()
            },
            egui::FontId::proportional(10.0),
            color,
        );
        x += preset.label().len() as f32 * 5.4 + if active { 46.0 } else { 22.0 };
    }
}

fn help_topic_for_workspace(workspace: Workspace) -> help::HelpTopic {
    match workspace {
        Workspace::Terrain => help::HelpTopic::Terrain,
        Workspace::Modifiers => help::HelpTopic::Modifiers,
        Workspace::Rock => help::HelpTopic::Rock,
        Workspace::Detail => help::HelpTopic::Detail,
        Workspace::Sculpt => help::HelpTopic::Sculpt,
        Workspace::Animation => help::HelpTopic::Path,
        Workspace::Erosion => help::HelpTopic::Erosion,
        Workspace::Water => help::HelpTopic::Water,
        Workspace::Materials => help::HelpTopic::Materials,
        Workspace::Light => help::HelpTopic::Light,
        Workspace::Camera => help::HelpTopic::Camera,
        Workspace::Render => help::HelpTopic::RenderFinal,
        Workspace::Files => help::HelpTopic::Export,
        Workspace::Help => help::HelpTopic::Help,
    }
}

fn help_topic_for_tab(tab: animation_ui::AnimTab) -> help::HelpTopic {
    match tab {
        animation_ui::AnimTab::Path => help::HelpTopic::Path,
        animation_ui::AnimTab::Timing => help::HelpTopic::Timing,
        animation_ui::AnimTab::Profile => help::HelpTopic::Profile,
        animation_ui::AnimTab::Preview => help::HelpTopic::Preview,
    }
}

fn target_mode_label(mode: TargetMode) -> &'static str {
    match mode {
        TargetMode::Fixed => "Fixe",
        TargetMode::EyeAhead => "Avance sur courbe",
        TargetMode::Path => "Tracé animé",
        TargetMode::Offset => "Décalage",
        TargetMode::Trail => "Retard sur courbe",
        TargetMode::Manual => "Manuel",
        TargetMode::None => "Aucune cible",
    }
}

fn quality_label(quality: Quality) -> &'static str {
    match quality {
        Quality::Low => "Low",
        Quality::Medium => "Medium",
        Quality::High => "High",
        Quality::Ultra => "Ultra",
        Quality::Custom => "Custom",
    }
}

/// Write a screenshot produced by `egui::Context::request_screenshot`.
fn save_capture_image(path: &Path, image: &egui::ColorImage) {
    let width = image.size[0] as u32;
    let height = image.size[1] as u32;
    let mut bytes = Vec::with_capacity(width as usize * height as usize * 4);
    for pixel in &image.pixels {
        bytes.extend_from_slice(&[pixel.r(), pixel.g(), pixel.b(), pixel.a()]);
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) =
        image::save_buffer(path, &bytes, width, height, image::ExtendedColorType::Rgba8)
    {
        eprintln!("capture {} : {error}", path.display());
    } else {
        println!("capture {}", path.display());
    }
}

#[cfg(test)]
mod preview_tests {
    use super::*;

    #[test]
    fn slow_renders_lower_the_interactive_scale() {
        let next = adapted_scale(0.6, 0.6, 200);
        assert!(next < 0.6);
        assert!(next >= MIN_PREVIEW_SCALE);
    }

    #[test]
    fn fast_renders_raise_the_interactive_scale() {
        let next = adapted_scale(0.3, 0.3, 10);
        assert!(next > 0.3);
        assert!(next <= MAX_INTERACTIVE_SCALE);
    }

    #[test]
    fn adaptation_ignores_zero_time() {
        assert!((adapted_scale(0.43, 0.43, 0) - 0.43).abs() < f32::EPSILON);
    }

    #[test]
    fn refinement_reaches_full_resolution() {
        let mut scale = MIN_PREVIEW_SCALE;
        for _ in 0..8 {
            scale = next_preview_scale(scale);
        }
        assert!((scale - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn workspace_and_tab_names_are_parsed_for_captures() {
        assert_eq!(Workspace::parse("Animation"), Some(Workspace::Animation));
        assert_eq!(Workspace::parse("matières"), Some(Workspace::Materials));
        assert_eq!(Workspace::parse("nope"), None);
        assert_eq!(
            animation_ui::AnimTab::parse("Timing"),
            Some(animation_ui::AnimTab::Timing)
        );
        assert_eq!(
            animation_ui::AnimTab::parse("3 · Profil"),
            Some(animation_ui::AnimTab::Profile)
        );
    }

    #[test]
    fn refinement_is_strictly_monotonic() {
        let mut scale = MIN_PREVIEW_SCALE;
        while scale < 0.999 {
            let next = next_preview_scale(scale);
            assert!(next > scale);
            scale = next;
        }
        assert!((scale - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn preview_quality_levels_are_ordered() {
        assert!(PreviewQuality::Draft.scale() < PreviewQuality::Normal.scale());
        assert!(PreviewQuality::Normal.scale() < PreviewQuality::High.scale());
        assert_eq!(PreviewQuality::High.scale(), 1.0);
        assert_eq!(PreviewQuality::Final.scale(), 1.0);
        assert_eq!(PreviewQuality::ALL.len(), 4);
    }

    #[test]
    fn paint_brush_and_line_modify_the_texture() {
        let mut paint = PaintState::new();
        paint.brush_radius = 6.0;
        paint.strength = 1.0;
        paint.color = [255, 0, 0];
        let far_before = paint.pixels[(200 * paint.width + 200) * 4];
        paint.paint(40, 40);
        let index = (40 * paint.width + 40) * 4;
        assert_eq!(&paint.pixels[index..index + 3], &[255, 0, 0]);
        assert_eq!(paint.pixels[(200 * paint.width + 200) * 4], far_before);
        let mid = (40 * paint.width + 90) * 4;
        paint.paint_line((40, 40), (100, 40));
        assert_eq!(paint.pixels[mid], 255);
    }

    #[test]
    fn painted_texture_is_saved_and_reloaded() {
        let mut paint = PaintState::new();
        paint.paint(64, 64);
        let dir = std::env::temp_dir().join(format!(
            "vf-paint-test-{}-{}",
            std::process::id(),
            std::line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        paint.path = dir.join("rock.png").display().to_string();
        let path = paint.save(None).unwrap();
        let image = image::open(&path).unwrap().to_rgba8();
        assert_eq!((image.width(), image.height()), (256, 256));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
