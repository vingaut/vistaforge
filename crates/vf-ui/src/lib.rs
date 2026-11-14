//! Desktop interface for VistaForge.
//!
//! Layout: a toolbar (project/render), a control panel (terrain, scene, camera,
//! output) and an interactive viewport. Rendering happens on a worker thread so
//! the UI stays responsive; changes trigger a debounced low-resolution preview.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui;
use vf_core::{
    AircraftPreset, CameraLink, ClassicParams, ErosionParams, HeightField, ModifierKind, Palette,
    PathKey, Project, Quality, RenderMode, Seed, TargetMode, TerrainSource,
};
use vf_io::RgbaImage;
use vf_render::{RenderOptions, RenderOutput, render_with_options};
use vf_terrain::HillshadeParams;
use vf_terrain::brush::{
    Brush, BrushMode, BrushShape, BrushUndo, Falloff, Stroke, Symmetry,
};

const PREVIEW_SIZE: (u32, u32) = (960, 540);
const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(350);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Workspace {
    Terrain,
    Modifiers,
    Rock,
    Detail,
    Sculpt,
    Rig,
    Erosion,
    Water,
    Materials,
    Light,
    Camera,
    Flight,
    Render,
}

impl Workspace {
    const ALL: [Workspace; 13] = [
        Self::Terrain,
        Self::Modifiers,
        Self::Rock,
        Self::Detail,
        Self::Sculpt,
        Self::Rig,
        Self::Erosion,
        Self::Water,
        Self::Materials,
        Self::Light,
        Self::Camera,
        Self::Flight,
        Self::Render,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Terrain => "Terrain",
            Self::Modifiers => "Modificateurs",
            Self::Rock => "Roche",
            Self::Detail => "Détail",
            Self::Sculpt => "Sculpter",
            Self::Rig => "Animation",
            Self::Erosion => "Érosion",
            Self::Water => "Eau",
            Self::Materials => "Matières",
            Self::Light => "Lumière",
            Self::Camera => "Caméra",
            Self::Flight => "Vol",
            Self::Render => "Rendu",
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
}

pub fn run(initial: Option<PathBuf>) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1000.0, 640.0])
            .with_title("VistaForge"),
        ..Default::default()
    };
    eframe::run_native(
        "VistaForge",
        options,
        Box::new(move |cc| Ok(Box::new(VistaForgeApp::new(cc, initial)))),
    )
}

struct VistaForgeApp {
    project: Project,
    heightfield: HeightField,
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
    fov: f32,

    render_width: u32,
    render_height: u32,
    quality: Quality,
    shadows: bool,
    ao: bool,
    clouds: bool,
    water: bool,
    exposure: f32,
    flight_time: f32,
    selected_modifier: usize,
    add_modifier_kind: ModifierKind,

    sculpt_brush: Brush,
    sculpt_stroke: Option<Stroke>,
    sculpt_undo: Vec<BrushUndo>,
    sculpt_redo: Vec<BrushUndo>,
    sculpt_texture: Option<egui::TextureHandle>,
    sculpt_texture_dirty: bool,
    sculpt_zoom: f32,
    sculpt_pan: egui::Vec2,
    sculpt_cursor: Option<(f64, f64)>,
    sculpt_status: String,

    rig_time: f32,
    rig_selected_key: Option<usize>,
    rig_zoom: f32,
    rig_pan: egui::Vec2,
    rig_cursor: Option<(f64, f64)>,
    rig_status: String,
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
        let mut app = Self {
            project,
            heightfield,
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
            fov: 48.0,
            render_width: 1280,
            render_height: 720,
            quality: Quality::High,
            shadows: true,
            ao: false,
            clouds: true,
            water: true,
            exposure: 1.0,
            flight_time: 0.0,
            selected_modifier: 0,
            add_modifier_kind: ModifierKind::Displace,
            sculpt_brush: Brush::default(),
            sculpt_stroke: None,
            sculpt_undo: Vec::new(),
            sculpt_redo: Vec::new(),
            sculpt_texture: None,
            sculpt_texture_dirty: true,
            sculpt_zoom: 1.0,
            sculpt_pan: egui::Vec2::ZERO,
            sculpt_cursor: None,
            sculpt_status: "Prêt. Clic gauche = sculpter, clic droit = déplacer.".into(),
            rig_time: 0.0,
            rig_selected_key: None,
            rig_zoom: 1.0,
            rig_pan: egui::Vec2::ZERO,
            rig_cursor: None,
            rig_status: "Tracez la courbe du point de vue, puis poursuivez la cible.".into(),
        };
        app.sync_from_project();
        app.sync_orbit_from_camera();
        app.start_render(cc.egui_ctx.clone(), false);
        app
    }

    fn sync_from_project(&mut self) {
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
        self.render_width = self.project.render.width.clamp(320, 3840);
        self.render_height = self.project.render.height.clamp(240, 2160);
        self.selected_modifier = self
            .selected_modifier
            .min(self.project.modifiers.items.len());
    }

    fn sync_orbit_from_camera(&mut self) {
        let center = vf_scene::terrain_center(&self.heightfield);
        let delta = self.project.camera.position - center;
        self.distance = delta.length().max(1.0);
        self.yaw = (delta.x.atan2(-delta.y).to_degrees()) as f32;
        self.pitch = (delta.z / self.distance).asin().to_degrees() as f32;
        self.rock_preset = self.project.geology.preset.clone();
        self.fov = self.project.camera.fov_y_deg;
    }

    fn apply_orbit(&mut self) {
        let center = vf_scene::terrain_center(&self.heightfield);
        self.project.camera =
            vf_scene::orbit(center, self.distance, self.yaw, self.pitch, self.fov);
    }

    fn auto_frame(&mut self) {
        let aspect = f64::from(self.render_width.max(1)) / f64::from(self.render_height.max(1));
        self.project.camera = vf_scene::auto_frame(&self.heightfield, aspect);
        self.sync_orbit_from_camera();
    }

    fn mark_dirty(&mut self) {
        self.dirty_since = Some(Instant::now());
    }

    fn new_project(&mut self, ctx: egui::Context) {
        let (project, heightfield) = default_scene();
        self.project = project;
        self.heightfield = heightfield;
        self.project_path = None;
        self.path_input = "landscape.vfproj".into();
        self.status = "New project".into();
        self.texture = None;
        self.last_image = None;
        self.sculpt_texture = None;
        self.sculpt_texture_dirty = true;
        self.sculpt_undo.clear();
        self.sculpt_redo.clear();
        self.sync_from_project();
        self.sync_orbit_from_camera();
        self.start_render(ctx, false);
    }

    fn open_project(&mut self, ctx: egui::Context) {
        let path = PathBuf::from(self.path_input.trim());
        match vf_io::load_project(&path) {
            Ok(loaded) => {
                self.project = loaded.project;
                self.heightfield = loaded.heightfield;
                self.project_path = Some(path.clone());
                self.path_input = path.display().to_string();
                self.status = format!("Loaded {}", path.display());
                self.texture = None;
                self.last_image = loaded.preview;
                self.sculpt_texture = None;
                self.sculpt_texture_dirty = true;
                self.sculpt_undo.clear();
                self.sculpt_redo.clear();
                self.sync_from_project();
                self.sync_orbit_from_camera();
                self.start_render(ctx, false);
            }
            Err(error) => self.status = format!("Open failed: {error}"),
        }
    }

    fn save_project(&mut self) {
        let path = PathBuf::from(self.path_input.trim());
        if path.as_os_str().is_empty() {
            self.status = "Save failed: empty path".into();
            return;
        }
        match vf_io::save_project(
            &self.project,
            &self.heightfield,
            &path,
            self.last_image.as_ref(),
        ) {
            Ok(()) => {
                self.project_path = Some(path.clone());
                self.status = format!("Saved {}", path.display());
            }
            Err(error) => self.status = format!("Save failed: {error}"),
        }
    }

    fn save_render(&mut self) {
        let Some(image) = &self.last_image else {
            self.status = "Nothing to save: render first".into();
            return;
        };
        match image.save_png(Path::new(self.render_path.trim())) {
            Ok(()) => self.status = format!("Wrote {}", self.render_path),
            Err(error) => self.status = format!("PNG save failed: {error}"),
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
        let started = Instant::now();
        let generated =
            vf_geology::build(&project.source, erosion.as_ref(), &project.geology, seed);
        match generated {
            Ok(heightfield) => {
                project.sync_terrain(&heightfield);
                project.camera = vf_scene::auto_frame(
                    &heightfield,
                    f64::from(self.render_width) / f64::from(self.render_height.max(1)),
                );
                self.project = project;
                self.heightfield = heightfield;
                self.texture = None;
                self.last_image = None;
                self.sculpt_texture = None;
                self.sculpt_texture_dirty = true;
                self.sculpt_undo.clear();
                self.sculpt_redo.clear();
                self.status = format!(
                    "Generated {} {}x{} seed {} in {} ms",
                    self.mode.label(),
                    size,
                    size,
                    self.seed,
                    started.elapsed().as_millis()
                );
                self.sync_orbit_from_camera();
                self.start_render(ctx, false);
            }
            Err(error) => self.status = format!("Generation failed: {error}"),
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

    fn preview_options(&self) -> RenderOptions {
        let mut options = RenderOptions::preview(PREVIEW_SIZE.0, PREVIEW_SIZE.1);
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

    fn start_render(&mut self, ctx: egui::Context, full: bool) {
        if self.render_task.is_some() {
            return;
        }
        let options = if full {
            self.full_options()
        } else {
            self.preview_options()
        };
        let project = self.project.clone();
        let heightfield = self.heightfield.clone();
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
        });
        self.status = if full {
            format!("Rendering {width}x{height}…")
        } else {
            "Rendering preview…".into()
        };
    }

    fn poll_render(&mut self, ctx: &egui::Context) {
        let Some(task) = self.render_task.take() else {
            return;
        };
        match task.receiver.try_recv() {
            Ok(Ok(output)) => {
                self.apply_render(ctx, output);
                self.last_render_ms = task.started.elapsed().as_millis();
            }
            Ok(Err(error)) => {
                self.status = format!("Render failed: {error}");
            }
            Err(TryRecvError::Empty) => {
                self.render_task = Some(task);
                ctx.request_repaint_after(Duration::from_millis(80));
            }
            Err(TryRecvError::Disconnected) => {
                self.status = "Render worker disconnected".into();
            }
        }
    }

    fn apply_render(&mut self, ctx: &egui::Context, output: RenderOutput) {
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [output.image.width as usize, output.image.height as usize],
            &output.image.pixels,
        );
        self.texture = Some(ctx.load_texture("viewport", image, egui::TextureOptions::LINEAR));
        self.last_render_size = (output.image.width, output.image.height);
        self.status = format!(
            "Rendered {}x{} (ssaa {}) in {} ms",
            output.stats.width, output.stats.height, output.stats.ssaa, output.stats.elapsed_ms
        );
        self.last_image = Some(output.image);
    }

    fn terrain_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::CollapsingHeader::new("Terrain")
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
        self.world_ui(ui, ctx);
    }

    fn world_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::CollapsingHeader::new("Monde et tuilage")
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
                    match vf_terrain::world::compose(
                        &self.heightfield,
                        &self.project.world,
                        self.project.seed,
                    ) {
                        Ok(composed) => {
                            self.heightfield = composed;
                            self.project.sync_terrain(&self.heightfield);
                            self.project.camera = vf_scene::auto_frame(
                                &self.heightfield,
                                f64::from(self.render_width.max(1))
                                    / f64::from(self.render_height.max(1)),
                            );
                            self.texture = None;
                            self.last_image = None;
                            self.sculpt_texture = None;
                            self.sculpt_texture_dirty = true;
                            self.status = format!(
                                "Monde composé : {}×{}",
                                self.heightfield.width(),
                                self.heightfield.height()
                            );
                            self.mark_dirty();
                            self.start_render(ctx.clone(), false);
                        }
                        Err(error) => self.status = format!("Monde : {error}"),
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
    }

    fn bake_modifiers(&mut self) {
        if self.project.modifiers.is_empty() {
            self.status = "Modifier stack is empty".into();
            return;
        }
        let stack = self.project.modifiers.clone();
        match vf_modifiers::apply(&self.heightfield, &stack, self.project.seed) {
            Ok(heightfield) => {
                self.heightfield = heightfield;
                self.project.modifiers.clear();
                self.selected_modifier = 0;
                self.project.sync_terrain(&self.heightfield);
                self.texture = None;
                self.last_image = None;
                self.status = "Modifiers baked into the heightfield".into();
                self.mark_dirty();
            }
            Err(error) => self.status = format!("Bake failed: {error}"),
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

        egui::CollapsingHeader::new("Pile de modificateurs")
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
                        self.bake_modifiers();
                    }
                    if ui.button("Vider").clicked() {
                        self.project.modifiers.clear();
                        self.selected_modifier = 0;
                        changed = true;
                    }
                    if ui.button("Aperçu").clicked() {
                        self.start_render(ctx.clone(), false);
                    }
                });
            });

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
        egui::CollapsingHeader::new("Géométrie")
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
                    self.start_render(ctx.clone(), false);
                }
            });

        egui::CollapsingHeader::new("Sillons et arêtes")
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

        egui::CollapsingHeader::new("Cratères, plaques, vésicules")
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
                ui.checkbox(
                    &mut self.project.detail.exfoliation.enabled,
                    "Exfoliation",
                );
                ui.checkbox(&mut self.project.detail.vesicles.enabled, "Vésicules");
            });

        egui::CollapsingHeader::new("Gros plan")
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
    }

    fn rock_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::CollapsingHeader::new("Famille de roche")
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
        egui::CollapsingHeader::new("Bancs")
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
        egui::CollapsingHeader::new("Fractures")
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
        egui::CollapsingHeader::new("Dissolution")
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
        egui::CollapsingHeader::new("Canyon")
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
        egui::CollapsingHeader::new("Soleil et ciel")
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
        egui::CollapsingHeader::new("Atmosphère")
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
    }

    fn water_ui(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Mer et eau")
            .default_open(true)
            .show(ui, |ui| {
                ui.checkbox(&mut self.project.water.enabled, "Eau activée");
                let (min, max) = self.heightfield.min_max();
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
    }

    fn materials_ui(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Matières")
            .default_open(true)
            .show(ui, |ui| {
                ui.checkbox(&mut self.project.materials.enabled, "Matériaux activés");
                let m = &mut self.project.materials;
                let mut changed = false;
                changed |= param_f32(ui, "Roche", "", &mut m.rock, 0.0, 1.0, 0.8);
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
        egui::CollapsingHeader::new("Étagement")
            .default_open(false)
            .show(ui, |ui| {
                let (min, max) = self.heightfield.min_max();
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
    }

    fn erosion_ui(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Érosion")
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
    }

    fn flight_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::CollapsingHeader::new("Survol")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Preset");
                    let mut selected = self.project.flight.preset.clone();
                    egui::ComboBox::from_id_salt("flight-preset")
                        .selected_text(&self.project.flight.preset)
                        .show_ui(ui, |ui| {
                            for name in vf_core::FLIGHT_PRESETS {
                                ui.selectable_value(&mut selected, (*name).to_string(), *name);
                            }
                        });
                    if selected != self.project.flight.preset
                        && let Some(settings) = vf_core::FlightSettings::preset(&selected)
                    {
                        self.flight_time = self.flight_time.min(settings.duration_s).max(0.0);
                        self.project.flight = settings;
                        self.mark_dirty();
                    }
                });
                ui.checkbox(&mut self.project.flight.enabled, "Vol actif");
                ui.horizontal(|ui| {
                    ui.label("Trajectoire");
                    let mut pattern = self.project.flight.pattern;
                    egui::ComboBox::from_id_salt("flight-pattern")
                        .selected_text(match pattern {
                            vf_core::FlightPattern::Orbit => "orbite",
                            vf_core::FlightPattern::Racetrack => "circuit",
                            vf_core::FlightPattern::RidgeRun => "crête",
                            vf_core::FlightPattern::Dive => "piqué",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut pattern,
                                vf_core::FlightPattern::Orbit,
                                "orbite",
                            );
                            ui.selectable_value(
                                &mut pattern,
                                vf_core::FlightPattern::Racetrack,
                                "circuit",
                            );
                            ui.selectable_value(
                                &mut pattern,
                                vf_core::FlightPattern::RidgeRun,
                                "crête",
                            );
                            ui.selectable_value(
                                &mut pattern,
                                vf_core::FlightPattern::Dive,
                                "piqué",
                            );
                        });
                    if pattern != self.project.flight.pattern {
                        self.project.flight.pattern = pattern;
                        self.mark_dirty();
                    }
                });
                let f = &mut self.project.flight;
                let mut changed = false;
                changed |= param_f32(ui, "Durée", "s", &mut f.duration_s, 0.5, 120.0, 8.0);
                changed |= param_u32(ui, "Images/s", "", &mut f.fps, 1, 60, 24);
                changed |= param_f32(ui, "Vitesse", "m/s", &mut f.speed_mps, 0.5, 90.0, 18.0);
                changed |= param_f32(
                    ui,
                    "Altitude sol",
                    "m",
                    &mut f.altitude_agl_m,
                    1.0,
                    1000.0,
                    28.0,
                );
                changed |= param_f32(
                    ui,
                    "Inclinaison",
                    "°",
                    &mut f.pitch_bias_deg,
                    -45.0,
                    30.0,
                    0.0,
                );
                changed |= param_f32(ui, "Virage max", "°", &mut f.max_bank_deg, 0.0, 85.0, 45.0);
                changed |= param_f32(ui, "Réactivité", "", &mut f.smoothness, 0.0, 1.0, 0.28);
                changed |= param_f32(ui, "Uptilt", "°", &mut f.uptilt_deg, 0.0, 60.0, 30.0);
                changed |= param_f32(ui, "Secousses", "°", &mut f.shake_deg, 0.0, 2.0, 0.12);
                changed |= param_f32(ui, "Turbulence", "", &mut f.turbulence, 0.0, 1.0, 0.35);
                let mut gimbal = f.gimbal;
                if ui.checkbox(&mut gimbal, "Nacelle stabilisée").changed() {
                    f.gimbal = gimbal;
                    changed = true;
                }
                if changed {
                    self.status = "Réglages de vol modifiés".into();
                }
            });

        egui::CollapsingHeader::new("Aperçu")
            .default_open(true)
            .show(ui, |ui| {
                let duration = self.project.flight.duration_s.max(0.5);
                let mut t = self.flight_time.clamp(0.0, duration);
                if ui
                    .add(
                        egui::Slider::new(&mut t, 0.0..=duration)
                            .text("Temps de vol (s)")
                            .fixed_decimals(2),
                    )
                    .changed()
                {
                    self.flight_time = t;
                    self.apply_flight_camera(ctx);
                }
                if ui
                    .add_sized(
                        [ui.available_width(), 26.0],
                        egui::Button::new("Aperçu à ce temps"),
                    )
                    .clicked()
                {
                    self.apply_flight_camera(ctx);
                }
                ui.label(
                    egui::RichText::new(
                        "La séquence complète se rend en CLI :\n                         vistaforge anim render <projet> <dossier> --preset cinematic",
                    )
                    .small()
                    .weak(),
                );
            });
    }

    fn apply_flight_camera(&mut self, ctx: &egui::Context) {
        if !self.project.flight.enabled {
            self.status = "Active d'abord le vol".into();
            return;
        }
        let seed = Seed::new(self.project.flight.seed);
        match vf_flight::sample_at(
            &self.project.flight,
            &self.heightfield,
            seed,
            self.project.camera.fov_y_deg,
            f64::from(self.flight_time),
        ) {
            Ok(sample) => {
                self.project.camera = sample.camera;
                self.status = format!(
                    "Vol t={:.2}s · {:.0} m sol · {:.1}° de gîte",
                    self.flight_time, sample.altitude_agl_m, sample.bank_deg
                );
                self.start_render(ctx.clone(), false);
            }
            Err(error) => self.status = format!("Vol: {error}"),
        }
    }

    fn workspace_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            match self.workspace {
                Workspace::Terrain => self.terrain_ui(ui, ctx),
                Workspace::Modifiers => self.modifiers_ui(ui, ctx),
                Workspace::Rock => self.rock_ui(ui, ctx),
                Workspace::Detail => self.detail_ui(ui, ctx),
                Workspace::Sculpt => self.sculpt_panel_ui(ui, ctx),
                Workspace::Rig => self.rig_panel_ui(ui, ctx),
                Workspace::Erosion => self.erosion_ui(ui),
                Workspace::Water => self.water_ui(ui),
                Workspace::Materials => self.materials_ui(ui),
                Workspace::Light => self.light_ui(ui),
                Workspace::Camera => self.camera_ui(ui),
                Workspace::Flight => self.flight_ui(ui, ctx),
                Workspace::Render => self.render_ui(ui, ctx),
            }
            ui.add_space(12.0);
            let stats = self.heightfield.stats();
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

    fn camera_ui(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Camera")
            .default_open(true)
            .show(ui, |ui| {
                if ui.button("Auto-frame terrain").clicked() {
                    self.auto_frame();
                    self.mark_dirty();
                }
                if ui
                    .add(egui::Slider::new(&mut self.yaw, -180.0..=180.0).text("Yaw"))
                    .changed()
                {
                    self.apply_orbit();
                    self.mark_dirty();
                }
                if ui
                    .add(egui::Slider::new(&mut self.pitch, -20.0..=85.0).text("Pitch"))
                    .changed()
                {
                    self.apply_orbit();
                    self.mark_dirty();
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
                    self.mark_dirty();
                }
                if ui
                    .add(egui::Slider::new(&mut self.fov, 10.0..=110.0).text("FOV (°)"))
                    .changed()
                {
                    self.project.camera.fov_y_deg = self.fov;
                    self.apply_orbit();
                    self.mark_dirty();
                }
                ui.label(
                    egui::RichText::new(format!(
                        "eye ({:.0}, {:.0}, {:.0})",
                        self.project.camera.position.x,
                        self.project.camera.position.y,
                        self.project.camera.position.z
                    ))
                    .small()
                    .weak(),
                );
            });
    }

    fn render_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::CollapsingHeader::new("Output")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Size");
                    ui.add(
                        egui::DragValue::new(&mut self.render_width)
                            .speed(8.0)
                            .range(64..=8192),
                    );
                    ui.label("x");
                    ui.add(
                        egui::DragValue::new(&mut self.render_height)
                            .speed(8.0)
                            .range(64..=8192),
                    );
                });
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
                        self.shadows = self.project.render.shadows;
                        self.ao = self.project.render.ao;
                        self.mark_dirty();
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut self.shadows, "Shadows");
                    ui.checkbox(&mut self.ao, "AO");
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
                    ui.checkbox(&mut self.clouds, "Clouds");
                    ui.checkbox(&mut self.water, "Water");
                });
                ui.horizontal(|ui| {
                    if ui
                        .add_sized(
                            [ui.available_width() * 0.55, 28.0],
                            egui::Button::new("Render"),
                        )
                        .clicked()
                    {
                        self.start_render(ctx.clone(), true);
                    }
                    if ui.button("Preview").clicked() {
                        self.start_render(ctx.clone(), false);
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
                        "last: {}x{} · {} ms",
                        self.last_render_size.0, self.last_render_size.1, self.last_render_ms
                    ))
                    .small()
                    .weak(),
                );
                ui.separator();
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.render_path).desired_width(180.0));
                    if ui.button("Save PNG").clicked() {
                        self.save_render();
                    }
                });
            });
    }

    fn rig_panel_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let duration = self.project.rig.duration_s.max(0.1);
        self.rig_time = self.rig_time.clamp(0.0, duration);
        egui::CollapsingHeader::new("Point de vue")
            .default_open(true)
            .show(ui, |ui| {
                ui.checkbox(&mut self.project.rig.enabled, "Rig actif");
                ui.horizontal(|ui| {
                    ui.label("Aéronef");
                    let mut preset = self.project.rig.body.preset;
                    egui::ComboBox::from_id_salt("rig-body")
                        .selected_text(preset.label())
                        .show_ui(ui, |ui| {
                            for candidate in AircraftPreset::ALL {
                                ui.selectable_value(&mut preset, candidate, candidate.label());
                            }
                        });
                    if preset != self.project.rig.body.preset {
                        let body = vf_core::MotionProfile::preset(preset);
                        self.project.rig.body = body;
                        self.rig_status = format!("Profil {}", preset.label());
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    for (name, label) in [("orbit", "Orbite"), ("ridge", "Crête"), ("dive", "Piqué")] {
                        if ui.button(label).clicked() {
                            match vf_flight::rig::preset_path(
                                &mut self.project.rig,
                                &self.heightfield,
                                name,
                            ) {
                                Ok(()) => {
                                    self.rig_selected_key = None;
                                    self.rig_time = 0.0;
                                    self.project.mark_modified();
                                    self.rig_status = format!("Trajectoire {label}");
                                }
                                Err(error) => self.rig_status = error,
                            }
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Cible");
                    let mut mode = self.project.rig.target.mode;
                    egui::ComboBox::from_id_salt("rig-target")
                        .selected_text(match mode {
                            TargetMode::Fixed => "Fixe",
                            TargetMode::EyeAhead => "Avance",
                            TargetMode::Path => "Courbe",
                            TargetMode::Manual => "Manuelle",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut mode, TargetMode::Fixed, "Fixe");
                            ui.selectable_value(&mut mode, TargetMode::EyeAhead, "Avance");
                            ui.selectable_value(&mut mode, TargetMode::Path, "Courbe");
                            ui.selectable_value(&mut mode, TargetMode::Manual, "Manuelle");
                        });
                    self.project.rig.target.mode = mode;
                });
                if self.project.rig.target.mode == TargetMode::Fixed {
                    ui.horizontal(|ui| {
                        ui.label("Point");
                        for axis in 0..3 {
                            ui.add(
                                egui::DragValue::new(&mut self.project.rig.target.fixed[axis])
                                    .speed(10.0),
                            );
                        }
                    });
                }
                param_f32(
                    ui,
                    "Avance",
                    "s",
                    &mut self.project.rig.target.lead_s,
                    0.0,
                    3.0,
                    0.35,
                );
                param_f32(
                    ui,
                    "Lissage visée",
                    "",
                    &mut self.project.rig.target.smoothing,
                    0.0,
                    1.0,
                    0.35,
                );
            });

        egui::CollapsingHeader::new("Clés et temps")
            .default_open(true)
            .show(ui, |ui| {
                ui.add(
                    egui::Slider::new(&mut self.rig_time, 0.0..=duration)
                        .text("Temps (s)")
                        .fixed_decimals(2),
                );
                let mut clip_duration = self.project.rig.duration_s;
                if ui
                    .add(
                        egui::Slider::new(&mut clip_duration, 0.1..=600.0)
                            .text("Durée du clip (s)")
                            .fixed_decimals(2),
                    )
                    .changed()
                {
                    self.project.rig.duration_s = clip_duration;
                    self.rig_time = self.rig_time.min(clip_duration);
                    self.project.mark_modified();
                }
                ui.horizontal(|ui| {
                    if ui.button("Aperçu à ce temps").clicked() {
                        self.apply_rig_camera(ctx.clone());
                    }
                    if ui.button("+ Clé ici").clicked() {
                        let (x, y) = self.rig_cursor.unwrap_or_else(|| {
                            let b = self.heightfield.bounds();
                            (
                                (b.min[0] + b.max[0]) * 0.5,
                                (b.min[1] + b.max[1]) * 0.5,
                            )
                        });
                        let z = self.terrain_height(x, y) as f64 + 180.0;
                        self.project.rig.eye.push(PathKey::new(self.rig_time, [x, y, z]));
                        self.rig_selected_key = None;
                        self.project.mark_modified();
                    }
                    if ui
                        .add_enabled(
                            self.project.rig.eye.keys.len() > 2,
                            egui::Button::new("Supprimer la clé"),
                        )
                        .clicked()
                        && let Some(index) = self.rig_selected_key
                        && index < self.project.rig.eye.keys.len()
                    {
                        self.project.rig.eye.keys.remove(index);
                        self.rig_selected_key = None;
                        self.project.mark_modified();
                    }
                });
                let mut selected = self.rig_selected_key;
                for (index, key) in self.project.rig.eye.keys.iter().enumerate() {
                    let label = format!(
                        "{index}. t={:.2}s  ({:.0}, {:.0}, {:.0})",
                        key.time_s, key.position[0], key.position[1], key.position[2]
                    );
                    if ui.selectable_label(selected == Some(index), label).clicked() {
                        selected = Some(index);
                    }
                }
                self.rig_selected_key = selected;
                if let Some(index) = self.rig_selected_key
                    && let Some(key) = self.project.rig.eye.keys.get_mut(index)
                {
                    ui.horizontal(|ui| {
                        ui.label("Temps de la clé");
                        ui.add(
                            egui::DragValue::new(&mut key.time_s)
                                .speed(0.05)
                                .range(0.0..=duration),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Altitude");
                        ui.add(egui::DragValue::new(&mut key.position[2]).speed(10.0));
                    });
                    ui.label(
                        egui::RichText::new("Glissez la clé sur la carte pour X/Y.")
                            .small()
                            .weak(),
                    );
                }
                ui.label(
                    egui::RichText::new(&self.rig_status)
                        .small()
                        .weak(),
                );
            });

        egui::CollapsingHeader::new("Optique et poursuite")
            .default_open(true)
            .show(ui, |ui| {
                param_f32(
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
                    param_f32(
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
                        param_f32(
                            ui,
                            "Distance de mise au point",
                            "m",
                            &mut self.project.rig.optics.focus_distance_m,
                            0.1,
                            100_000.0,
                            100.0,
                        );
                    }
                }
                param_f32(
                    ui,
                    "Roulis explicite",
                    "°",
                    &mut self.project.rig.optics.roll_deg,
                    -180.0,
                    180.0,
                    0.0,
                );
                ui.horizontal(|ui| {
                    ui.label("Liaison caméra");
                    let mut link = self.project.rig.optics.link;
                    egui::ComboBox::from_id_salt("rig-link")
                        .selected_text(match link {
                            CameraLink::Eye => "Rigide",
                            CameraLink::Spring => "Ressort",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut link, CameraLink::Eye, "Rigide");
                            ui.selectable_value(&mut link, CameraLink::Spring, "Ressort");
                        });
                    self.project.rig.optics.link = link;
                });
                if self.project.rig.optics.link == CameraLink::Spring {
                    param_f32(
                        ui,
                        "Raideur",
                        "",
                        &mut self.project.rig.optics.spring_stiffness,
                        0.1,
                        40.0,
                        6.0,
                    );
                    param_f32(
                        ui,
                        "Amortissement",
                        "",
                        &mut self.project.rig.optics.spring_damping,
                        0.0,
                        3.0,
                        0.8,
                    );
                }
            });

        egui::CollapsingHeader::new("Dynamique du corps")
            .default_open(false)
            .show(ui, |ui| {
                let body = &mut self.project.rig.body;
                param_f32(ui, "Vitesse max", "m/s", &mut body.max_speed_mps, 0.5, 300.0, 30.0);
                param_f32(
                    ui,
                    "Accél. avant",
                    "m/s²",
                    &mut body.accel_forward_mps2,
                    0.05,
                    200.0,
                    25.0,
                );
                param_f32(
                    ui,
                    "Accél. latérale",
                    "m/s²",
                    &mut body.accel_lateral_mps2,
                    0.05,
                    200.0,
                    30.0,
                );
                param_f32(
                    ui,
                    "Accél. verticale",
                    "m/s²",
                    &mut body.accel_vertical_mps2,
                    0.05,
                    200.0,
                    20.0,
                );
                param_f32(ui, "Jerk", "m/s³", &mut body.jerk_mps3, 0.1, 1000.0, 60.0);
                param_f32(ui, "Traînée", "", &mut body.drag, 0.0, 1.0, 0.12);
                param_f32(ui, "Gain de roulis", "", &mut body.bank_gain, 0.0, 2.0, 1.0);
                param_f32(
                    ui,
                    "Roulis max",
                    "°",
                    &mut body.max_bank_deg,
                    0.0,
                    89.0,
                    75.0,
                );
                param_f32(
                    ui,
                    "Réponse roulis",
                    "",
                    &mut body.roll_response,
                    0.1,
                    40.0,
                    10.0,
                );
                param_f32(
                    ui,
                    "Secousses",
                    "°",
                    &mut body.shake_deg,
                    0.0,
                    5.0,
                    0.1,
                );
                param_f32(
                    ui,
                    "Turbulence",
                    "",
                    &mut body.turbulence,
                    0.0,
                    1.0,
                    0.35,
                );
            });

        egui::CollapsingHeader::new("Contraintes")
            .default_open(false)
            .show(ui, |ui| {
                let constraints = &mut self.project.rig.constraints;
                ui.checkbox(&mut constraints.enabled, "Contraintes actives");
                param_f32(
                    ui,
                    "Hauteur sol",
                    "m",
                    &mut constraints.ground_clearance_m,
                    0.0,
                    2000.0,
                    25.0,
                );
                ui.checkbox(
                    &mut constraints.reference_enabled,
                    "Sol de référence fixe",
                );
                if constraints.reference_enabled {
                    param_f32(
                        ui,
                        "Altitude de référence",
                        "m",
                        &mut constraints.reference_z_m,
                        -10_000.0,
                        20_000.0,
                        0.0,
                    );
                }
                param_f32(
                    ui,
                    "Distance parois",
                    "m",
                    &mut constraints.wall_distance_m,
                    0.0,
                    2000.0,
                    40.0,
                );
                ui.checkbox(&mut constraints.wall_avoidance, "Évitement des parois");
                ui.checkbox(&mut constraints.apply_eye, "Appliquer au point de vue");
                ui.checkbox(&mut constraints.apply_camera, "Appliquer à la caméra");
                ui.checkbox(&mut constraints.apply_target, "Appliquer à la cible");
                param_f32(
                    ui,
                    "Douceur",
                    "",
                    &mut constraints.softness,
                    0.0,
                    1.0,
                    0.35,
                );
            });

        ui.add_space(6.0);
        if ui
            .add_sized(
                [ui.available_width(), 26.0],
                egui::Button::new("Rendre l'aperçu de ce temps"),
            )
            .clicked()
        {
            self.apply_rig_camera(ctx.clone());
        }
    }

    fn apply_rig_camera(&mut self, ctx: egui::Context) {
        match vf_flight::rig::sample_rig(
            &self.project.rig,
            &self.heightfield,
            self.project.seed,
            self.rig_time,
        ) {
            Ok(sample) => {
                self.project.camera = sample.camera;
                self.rig_status = format!(
                    "t={:.2}s · {:.1} m sol · gîte {:.1}° · {:.1} m/s",
                    self.rig_time, sample.altitude_agl_m, sample.bank_deg, sample.speed_mps
                );
                self.start_render(ctx, false);
            }
            Err(error) => self.rig_status = format!("Rig: {error}"),
        }
    }

    fn terrain_height(&self, x: f64, y: f64) -> f32 {
        let (ox, oy) = self.heightfield.origin();
        let (sx, sy) = self.heightfield.spacing();
        self.heightfield
            .sample_bilinear((x - ox) / sx, (y - oy) / sy)
    }

    fn rig_canvas_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.ensure_hillshade(ctx);
        let available = ui.available_size();
        let (rect, response) =
            ui.allocate_exact_size(available, egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(12, 15, 19));

        let bounds = self.heightfield.bounds();
        let world_w = (bounds.max[0] - bounds.min[0]).max(1.0);
        let world_h = (bounds.max[1] - bounds.min[1]).max(1.0);
        let fit = ((rect.width() as f64 / world_w).min(rect.height() as f64 / world_h) * 0.92)
            as f32
            * self.rig_zoom;
        let image_size = egui::vec2(world_w as f32 * fit, world_h as f32 * fit);
        let center = rect.center() + self.rig_pan;
        let image_rect = egui::Rect::from_center_size(center, image_size);
        if let Some(texture) = &self.sculpt_texture {
            painter.image(
                texture.id(),
                image_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        }
        let to_screen = |world: [f64; 3]| -> egui::Pos2 {
            let u = ((world[0] - bounds.min[0]) / world_w) as f32;
            let v = ((bounds.max[1] - world[1]) / world_h) as f32;
            egui::pos2(
                image_rect.left() + u * image_size.x,
                image_rect.top() + v * image_size.y,
            )
        };
        let to_world = |screen: egui::Pos2| -> (f64, f64) {
            let u = ((screen.x - image_rect.left()) / image_size.x.max(1.0)).clamp(0.0, 1.0);
            let v = ((screen.y - image_rect.top()) / image_size.y.max(1.0)).clamp(0.0, 1.0);
            (
                bounds.min[0] + f64::from(u) * world_w,
                bounds.max[1] - f64::from(v) * world_h,
            )
        };

        if response.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > 0.1 {
                let old_zoom = self.rig_zoom;
                self.rig_zoom = (self.rig_zoom * (scroll * 0.0015).exp()).clamp(0.2, 40.0);
                if let Some(pointer) = response.hover_pos() {
                    let offset = pointer - center;
                    self.rig_pan += offset * (1.0 - self.rig_zoom / old_zoom);
                }
            }
        }
        if response.dragged_by(egui::PointerButton::Secondary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            self.rig_pan += response.drag_delta();
        }
        self.rig_cursor = response.hover_pos().map(to_world);

        // Eye path and target path.
        let duration = self.project.rig.duration_s.max(0.1);
        let mut previous: Option<egui::Pos2> = None;
        for i in 0..=160 {
            let t = duration * i as f32 / 160.0;
            let p = vf_flight::rig::sample_path(&self.project.rig.eye, t);
            let screen = to_screen(p.to_array());
            if let Some(prev) = previous {
                painter.line_segment(
                    [prev, screen],
                    egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 190, 90)),
                );
            }
            previous = Some(screen);
        }
        if self.project.rig.target.mode == TargetMode::Path {
            let mut previous: Option<egui::Pos2> = None;
            for i in 0..=80 {
                let t = duration * i as f32 / 80.0;
                let p = vf_flight::rig::sample_path(&self.project.rig.target.path, t);
                let screen = to_screen(p.to_array());
                if let Some(prev) = previous {
                    painter.line_segment(
                        [prev, screen],
                        egui::Stroke::new(1.5, egui::Color32::from_rgb(90, 200, 255)),
                    );
                }
                previous = Some(screen);
            }
        }

        // Keys.
        let mut hit: Option<usize> = None;
        for (index, key) in self.project.rig.eye.keys.iter().enumerate() {
            let screen = to_screen(key.position);
            let selected = self.rig_selected_key == Some(index);
            let radius = if selected { 7.0 } else { 5.0 };
            painter.circle_filled(
                screen,
                radius,
                if selected {
                    egui::Color32::from_rgb(255, 230, 140)
                } else {
                    egui::Color32::from_rgb(255, 170, 70)
                },
            );
            painter.text(
                screen + egui::vec2(8.0, -8.0),
                egui::Align2::LEFT_TOP,
                format!("{:.1}s", key.time_s),
                egui::FontId::monospace(10.0),
                egui::Color32::from_rgb(230, 220, 190),
            );
            if let Some(pointer) = response.hover_pos()
                && pointer.distance(screen) < 12.0
            {
                hit = Some(index);
            }
        }

        if ui.input(|input| input.pointer.primary_pressed()) && response.contains_pointer() {
            self.rig_selected_key = hit;
        }
        if response.dragged_by(egui::PointerButton::Primary)
            && let Some(pointer) = response.hover_pos()
            && let Some(index) = self.rig_selected_key
            && let Some(key) = self.project.rig.eye.keys.get_mut(index)
        {
            let (x, y) = to_world(pointer);
            key.position[0] = x;
            key.position[1] = y;
            self.project.mark_modified();
            self.rig_status = format!("Clé {index} déplacée en ({x:.0}, {y:.0})");
        }

        if let Some(world) = self.rig_cursor {
            let screen = to_screen([world.0, world.1, 0.0]);
            painter.circle_stroke(
                screen,
                9.0,
                egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 210, 120)),
            );
        }
        painter.text(
            rect.left_top() + egui::vec2(10.0, 10.0),
            egui::Align2::LEFT_TOP,
            format!(
                "Animation · t={:.2}s · {} clés · cible {:?} · zoom {:.1}×",
                self.rig_time,
                self.project.rig.eye.keys.len(),
                self.project.rig.target.mode,
                self.rig_zoom
            ),
            egui::FontId::proportional(13.0),
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 190),
        );
    }

    fn sculpt_panel_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::CollapsingHeader::new("Pinceau")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Mode");
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
                param_f32(
                    ui,
                    "Intensité",
                    if matches!(
                        self.sculpt_brush.mode,
                        BrushMode::Raise
                            | BrushMode::Lower
                            | BrushMode::Ridge
                            | BrushMode::Groove
                            | BrushMode::Crater
                            | BrushMode::Noise
                    ) {
                        "m"
                    } else {
                        ""
                    },
                    &mut self.sculpt_brush.strength,
                    0.0,
                    100.0,
                    8.0,
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
                        "Marche",
                        "m",
                        &mut self.sculpt_brush.step_m,
                        0.1,
                        500.0,
                        25.0,
                    );
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

        egui::CollapsingHeader::new("Symétrie et presets")
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
                });
            });

        egui::CollapsingHeader::new("Historique")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.sculpt_undo.is_empty(), egui::Button::new("↶ Annuler"))
                        .clicked()
                    {
                        self.sculpt_undo_action();
                    }
                    if ui
                        .add_enabled(!self.sculpt_redo.is_empty(), egui::Button::new("↷ Rétablir"))
                        .clicked()
                    {
                        self.sculpt_redo_action();
                    }
                });
                ui.label(
                    egui::RichText::new(format!(
                        "{} action(s) · {} Mo d'historique",
                        self.sculpt_undo.len(),
                        self.sculpt_undo.iter().map(|u| u.bytes()).sum::<usize>() / 1_048_576
                    ))
                    .small()
                    .weak(),
                );
                if ui.button("Vider l'historique").clicked() {
                    self.sculpt_undo.clear();
                    self.sculpt_redo.clear();
                }
                ui.label(
                    egui::RichText::new(&self.sculpt_status)
                        .small()
                        .weak(),
                );
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

        let _ = ctx;
    }

    fn ensure_hillshade(&mut self, ctx: &egui::Context) {
        if self.sculpt_texture.is_some() && !self.sculpt_texture_dirty {
            return;
        }
        let params = HillshadeParams {
            azimuth_deg_from_south: self.project.sun.azimuth_deg_from_south,
            declination_deg: self.project.sun.declination_deg,
            vertical_exaggeration: 2.0,
            ambient: 0.18,
        };
        let shaded = vf_terrain::hillshade(&self.heightfield, params);
        let image = egui::ColorImage::from_gray([shaded.width, shaded.height], &shaded.pixels);
        self.sculpt_texture = Some(ctx.load_texture(
            "sculpt-hillshade",
            image,
            egui::TextureOptions::LINEAR,
        ));
        self.sculpt_texture_dirty = false;
    }

    fn sculpt_canvas_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.ensure_hillshade(ctx);

        let available = ui.available_size();
        let (rect, response) =
            ui.allocate_exact_size(available, egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(12, 15, 19));

        let bounds = self.heightfield.bounds();
        let world_w = (bounds.max[0] - bounds.min[0]).max(1.0);
        let world_h = (bounds.max[1] - bounds.min[1]).max(1.0);
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
                bounds.min[0] + f64::from(u) * world_w,
                bounds.max[1] - f64::from(v) * world_h,
            )
        };
        let to_screen = |world: (f64, f64)| -> egui::Pos2 {
            let u = ((world.0 - bounds.min[0]) / world_w) as f32;
            let v = ((bounds.max[1] - world.1) / world_h) as f32;
            egui::pos2(
                image_rect.left() + u * image_size.x,
                image_rect.top() + v * image_size.y,
            )
        };

        if response.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > 0.1 {
                let old_zoom = self.sculpt_zoom;
                self.sculpt_zoom =
                    (self.sculpt_zoom * (scroll * 0.0015).exp()).clamp(0.2, 40.0);
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
            && response.contains_pointer()
            && let Some(pointer) = response.hover_pos()
        {
            let world = to_world(pointer);
            let mut stroke = Stroke::begin(
                &self.heightfield,
                self.project.seed.derive("sculpt"),
                "Sculpter",
            );
            stroke.stamp_to(&mut self.heightfield, &self.sculpt_brush, world.0, world.1);
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
            stroke.stamp_to(&mut self.heightfield, &self.sculpt_brush, world.0, world.1);
            self.sculpt_stroke = Some(stroke);
        }
        if primary_released
            && let Some(stroke) = self.sculpt_stroke.take()
            && let Some(undo) = stroke.finish(&self.heightfield)
        {
            let bytes = undo.bytes();
            self.sculpt_undo.push(undo);
            self.sculpt_redo.clear();
            self.sculpt_texture_dirty = true;
            self.project.mark_modified();
            self.mark_dirty();
            self.sculpt_status = format!(
                "Trait appliqué · {} action(s) · {} Ko",
                self.sculpt_undo.len(),
                bytes / 1024
            );
            self.start_render(ctx.clone(), false);
        }

        if let Some(world) = self.sculpt_cursor {
            let screen = to_screen(world);
            let radius_px = (f64::from(self.sculpt_brush.radius_m) * f64::from(fit)) as f32;
            let color = egui::Color32::from_rgb(255, 210, 120);
            painter.circle_stroke(screen, radius_px, egui::Stroke::new(1.5, color));
            painter.circle_stroke(screen, radius_px * 0.5, egui::Stroke::new(1.0, color.gamma_multiply(0.35)));
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

    fn sculpt_undo_action(&mut self) {
        if let Some(undo) = self.sculpt_undo.pop() {
            undo.undo(&mut self.heightfield);
            self.sculpt_redo.push(undo);
            self.sculpt_texture_dirty = true;
            self.project.mark_modified();
            self.mark_dirty();
            self.status = "Sculpter : annulé".into();
        }
    }

    fn sculpt_redo_action(&mut self) {
        if let Some(undo) = self.sculpt_redo.pop() {
            undo.redo(&mut self.heightfield);
            self.sculpt_undo.push(undo);
            self.sculpt_texture_dirty = true;
            self.project.mark_modified();
            self.mark_dirty();
            self.status = "Sculpter : rétabli".into();
        }
    }

    fn viewport_ui(&mut self, ui: &mut egui::Ui) {
        let available = ui.available_size();
        let Some(texture) = &self.texture else {
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
        let image_size = egui::vec2(texture.size()[0] as f32, texture.size()[1] as f32);
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
        if response.dragged() {
            let delta = response.drag_delta();
            self.yaw = (self.yaw + delta.x * 0.25).clamp(-180.0, 180.0);
            self.pitch = (self.pitch - delta.y * 0.25).clamp(-20.0, 85.0);
            self.apply_orbit();
            self.mark_dirty();
        }
        if response.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > 0.1 {
                self.distance =
                    (self.distance * (-f64::from(scroll) * 0.0015).exp()).clamp(100.0, 80_000.0);
                self.apply_orbit();
                self.mark_dirty();
            }
        }
        ui.painter().text(
            rect.left_top() + egui::vec2(10.0, 10.0),
            egui::Align2::LEFT_TOP,
            format!("{} · drag to orbit · wheel to zoom", self.project.meta.name),
            egui::FontId::proportional(13.0),
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, 170),
        );
    }
}

impl eframe::App for VistaForgeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_render(&ctx);

        if let Some(since) = self.dirty_since {
            if since.elapsed() >= PREVIEW_DEBOUNCE {
                if self.render_task.is_none() {
                    self.dirty_since = None;
                    self.start_render(ctx.clone(), false);
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(80));
            }
        }

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("VistaForge");
                ui.add_space(10.0);
                ui.separator();
                if ui.button("New").clicked() {
                    self.new_project(ctx.clone());
                }
                ui.add(egui::TextEdit::singleline(&mut self.path_input).desired_width(280.0));
                if ui.button("Open").clicked() {
                    self.open_project(ctx.clone());
                }
                if ui.button("Save").clicked() {
                    self.save_project();
                }
                ui.separator();
                if ui.button("Render").clicked() {
                    self.start_render(ctx.clone(), true);
                }
                if self.render_task.is_some() {
                    ui.spinner();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(&self.status).small());
                });
            });
        });

        egui::Panel::top("functions").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for workspace in Workspace::ALL {
                    let active = self.workspace == workspace;
                    let text = egui::RichText::new(workspace.label()).size(13.0);
                    if ui.selectable_label(active, text).clicked() {
                        self.workspace = workspace;
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

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(egui::Color32::from_rgb(16, 18, 22)))
            .show(ui, |ui| {
                if self.workspace == Workspace::Sculpt {
                    self.sculpt_canvas_ui(ui, &ctx);
                } else if self.workspace == Workspace::Rig {
                    self.rig_canvas_ui(ui, &ctx);
                } else {
                    self.viewport_ui(ui);
                }
            });
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

fn quality_label(quality: Quality) -> &'static str {
    match quality {
        Quality::Low => "Low",
        Quality::Medium => "Medium",
        Quality::High => "High",
        Quality::Ultra => "Ultra",
        Quality::Custom => "Custom",
    }
}
