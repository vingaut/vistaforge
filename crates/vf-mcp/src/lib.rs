//! Model Context Protocol server for VistaForge.
//!
//! Implements the MCP stdio transport (newline-delimited JSON-RPC 2.0),
//! `initialize`, `tools/list` and `tools/call`. All filesystem paths are
//! confined to a workspace root; no shell or network access is exposed.

use std::io::{BufRead, Cursor, Write};
use std::path::{Component, Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use vf_core::{
    ClassicParams, DemSource, ErosionParams, FlightSettings, GeologySettings, HeightField,
    LegacyMeta, ModernParams, Modifier, ModifierKind, Project, Quality, Seed, TerrainSource,
};
use vf_io::{DemFile, RgbaImage};
use vf_render::{RenderOptions, render_with_options};

pub const PROTOCOL_VERSION: &str = "2024-11-05";
pub const SERVER_NAME: &str = "vistaforge";

#[derive(Debug)]
struct Loaded {
    project: Project,
    heightfield: HeightField,
    path: Option<PathBuf>,
}

/// The heightfield the renderer and the tools should see: the stored base
/// terrain with the live modifier stack applied. Empty stacks return a clone
/// so callers do not need to special-case the two paths.
fn live_heightfield(loaded: &Loaded) -> std::result::Result<HeightField, String> {
    if loaded.project.modifiers.active().next().is_some() {
        vf_modifiers::apply(
            &loaded.heightfield,
            &loaded.project.modifiers,
            loaded.project.seed,
        )
        .map_err(|e| format!("modifier failed: {e}"))
    } else {
        Ok(loaded.heightfield.clone())
    }
}

pub struct Server {
    workspace: PathBuf,
    loaded: Option<Loaded>,
    last_preview: Option<RgbaImage>,
}

impl Server {
    pub fn new(workspace: PathBuf) -> Self {
        Self {
            workspace,
            loaded: None,
            last_preview: None,
        }
    }

    fn handle(&mut self, message: Value) -> Option<Value> {
        let id = message.get("id").cloned();
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let notification = id.is_none();

        let outcome: std::result::Result<Value, (i64, String)> = match method.as_str() {
            "initialize" => Ok(self.initialize()),
            "tools/list" => Ok(json!({ "tools": tool_defs() })),
            "tools/call" => self.call_tool(params),
            "resources/list" => Ok(json!({ "resources": [] })),
            "prompts/list" => Ok(json!({ "prompts": [] })),
            "ping" => Ok(json!({})),
            other if other.starts_with("notifications/") => {
                let _ = other;
                return None;
            }
            other => Err((-32601, format!("method not found: {other}"))),
        };

        if notification {
            return None;
        }
        let id = id.unwrap_or(Value::Null);
        Some(match outcome {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, message)) => {
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
            }
        })
    }

    fn initialize(&self) -> Value {
        json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": SERVER_NAME, "version": env!("CARGO_PKG_VERSION") },
            "instructions": "VistaForge generates, edits and renders 2.5D landscapes. \
                 Create a project with project.new, tweak terrain/camera/sun, then call render.still."
        })
    }

    fn call_tool(&mut self, params: Value) -> std::result::Result<Value, (i64, String)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or((-32602, "tools/call requires a name".to_string()))?
            .to_owned();
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        match self.execute(&name, arguments) {
            Ok(output) => {
                let mut content = vec![json!({ "type": "text", "text": output.text })];
                if let Some(png) = output.image {
                    content.push(json!({
                        "type": "image",
                        "data": STANDARD.encode(png),
                        "mimeType": "image/png"
                    }));
                }
                let mut result = json!({
                    "content": content,
                    "isError": false
                });
                if let Some(structured) = output.structured {
                    result["structuredContent"] = structured;
                }
                Ok(result)
            }
            Err(message) => Ok(json!({
                "content": [{ "type": "text", "text": message }],
                "isError": true
            })),
        }
    }

    fn execute(&mut self, name: &str, args: Value) -> std::result::Result<ToolOutput, String> {
        match name {
            "project.new" => self.project_new(args),
            "project.open" => self.project_open(args),
            "project.save" => self.project_save(args),
            "project.info" => self.project_info(),
            "project.list" => self.project_list(),
            "terrain.generate" => self.terrain_generate(args),
            "terrain.import_dem" => self.terrain_import_dem(args),
            "terrain.resample" => self.terrain_resample(args),
            "terrain.smooth" => self.terrain_smooth(args),
            "terrain.height_query" => self.terrain_height_query(args),
            "terrain.export_dem" => self.terrain_export_dem(args),
            "terrain.export_ascii" => self.terrain_export_ascii(args),
            "modifiers.kinds" => self.modifiers_kinds(),
            "modifiers.list" => self.modifiers_list(),
            "modifiers.add" => self.modifiers_add(args),
            "modifiers.set" => self.modifiers_set(args),
            "modifiers.remove" => self.modifiers_remove(args),
            "modifiers.clear" => self.modifiers_clear(),
            "modifiers.bake" => self.modifiers_bake(),
            "geology.set" => self.geology_set(args),
            "scene.preset" => self.scene_preset(args),
            "camera.set" => self.camera_set(args),
            "sun.set" => self.sun_set(args),
            "water.set" => self.water_set(args),
            "atmosphere.set" => self.atmosphere_set(args),
            "render.still" => self.render_still(args),
            "render.preview" => self.render_preview(args),
            "flight.info" => self.flight_info(args),
            "flight.still" => self.flight_still(args),
            other => Err(format!("unknown tool '{other}'")),
        }
    }

    fn workspace_path(&self, raw: &str) -> std::result::Result<PathBuf, String> {
        let path = Path::new(raw);
        if path.is_absolute() {
            return Err("absolute paths are not allowed".into());
        }
        if path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err("paths must stay inside the workspace".into());
        }
        Ok(self.workspace.join(path))
    }

    fn existing_path(&self, raw: &str) -> std::result::Result<PathBuf, String> {
        let path = self.workspace_path(raw)?;
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("cannot open '{}': {e}", path.display()))?;
        if !canonical.starts_with(&self.workspace) {
            return Err("path escapes the workspace".into());
        }
        Ok(canonical)
    }

    fn loaded_mut(&mut self) -> std::result::Result<&mut Loaded, String> {
        self.loaded
            .as_mut()
            .ok_or_else(|| "no project is open; call project.new or project.open first".into())
    }

    fn project_new(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize, Default)]
        struct Args {
            path: Option<String>,
            name: Option<String>,
            seed: Option<u64>,
            size: Option<u32>,
            mode: Option<String>,
            preset: Option<String>,
            erosion: Option<bool>,
        }
        let args: Args = parse_args(args)?;
        let seed = args.seed.unwrap_or(1);
        let size = args.size.unwrap_or(1026).clamp(2, 4096);
        let mode = args.mode.unwrap_or_else(|| "classic".into());
        let preset = args.preset.unwrap_or_else(|| "alpine".into());
        let name = args.name.unwrap_or_else(|| format!("VistaForge {seed}"));
        let path = match args.path {
            Some(path) => self.workspace_path(&path)?,
            None => self.workspace.join(format!("{}.vfproj", slug(&name))),
        };
        let (mut project, heightfield) = build_terrain(
            &name,
            seed,
            size,
            &mode,
            &preset,
            args.erosion.unwrap_or(false),
        )?;
        project.camera = auto_camera(&heightfield, &project);
        project.mark_modified();
        vf_io::save_project(&project, &heightfield, &path, None)
            .map_err(|e| format!("cannot save project: {e}"))?;
        let stats = heightfield.stats();
        let summary = format!(
            "created {} ({} mode, {}x{}, seed {})\nrelief {:.1} m",
            path.display(),
            project.source.kind(),
            heightfield.width(),
            heightfield.height(),
            seed,
            stats.relief()
        );
        self.loaded = Some(Loaded {
            project,
            heightfield,
            path: Some(path.clone()),
        });
        self.last_preview = None;
        Ok(ToolOutput::text(summary).with_structured(json!({
            "path": path,
            "name": name,
            "seed": seed,
            "size": size,
            "mode": mode,
            "stats": stats,
        })))
    }

    fn project_open(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize)]
        struct Args {
            path: String,
        }
        let args: Args = parse_args(args)?;
        let path = self.existing_path(&args.path)?;
        let loaded = vf_io::load_project(&path).map_err(|e| format!("cannot load project: {e}"))?;
        let summary = format!(
            "opened {} ({}x{}, seed {})",
            path.display(),
            loaded.heightfield.width(),
            loaded.heightfield.height(),
            loaded.project.seed
        );
        self.last_preview = loaded.preview.clone();
        self.loaded = Some(Loaded {
            project: loaded.project,
            heightfield: loaded.heightfield,
            path: Some(path.clone()),
        });
        Ok(ToolOutput::text(summary).with_structured(json!({ "path": path })))
    }

    fn project_save(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize)]
        struct Args {
            path: Option<String>,
        }
        let args: Args = parse_args(args)?;
        let preview = self.last_preview.clone();
        let workspace = self.workspace.clone();
        let loaded = self.loaded_mut()?;
        if let Some(raw) = args.path {
            let path = resolve_output(&workspace, &raw)?;
            loaded.path = Some(path);
        }
        let path = loaded
            .path
            .clone()
            .ok_or_else(|| "project has no path; provide one".to_string())?;
        vf_io::save_project(
            &loaded.project,
            &loaded.heightfield,
            &path,
            preview.as_ref(),
        )
        .map_err(|e| format!("cannot save project: {e}"))?;
        Ok(ToolOutput::text(format!("saved {}", path.display()))
            .with_structured(json!({ "path": path })))
    }

    fn project_info(&mut self) -> std::result::Result<ToolOutput, String> {
        let loaded = self.loaded_mut()?;
        let shaped = live_heightfield(loaded)?;
        let stats = shaped.stats();
        let info = json!({
            "path": loaded.path,
            "name": loaded.project.meta.name,
            "id": loaded.project.id.to_string(),
            "seed": loaded.project.seed.0,
            "source": loaded.project.source.kind(),
            "dimensions": [loaded.heightfield.width(), loaded.heightfield.height()],
            "spacing": [loaded.heightfield.spacing().0, loaded.heightfield.spacing().1],
            "stats": stats,
            "camera": {
                "position": loaded.project.camera.position.to_array(),
                "target": loaded.project.camera.target.to_array(),
                "fov": loaded.project.camera.fov_y_deg,
            },
            "sun": {
                "azimuth_from_south": loaded.project.sun.azimuth_deg_from_south,
                "declination": loaded.project.sun.declination_deg,
                "intensity": loaded.project.sun.intensity,
            },
            "water": {
                "enabled": loaded.project.water.enabled,
                "sea_level": loaded.project.water.sea_level,
            },
            "render": {
                "width": loaded.project.render.width,
                "height": loaded.project.render.height,
                "ssaa": loaded.project.render.ssaa,
            },
            "modifiers": {
                "count": loaded.project.modifiers.items.len(),
                "enabled": loaded.project.modifiers.active().count(),
                "summary": loaded.project.modifiers.summary(),
            }
        });
        Ok(
            ToolOutput::text(serde_json::to_string_pretty(&info).unwrap_or_default())
                .with_structured(info),
        )
    }

    fn project_list(&mut self) -> std::result::Result<ToolOutput, String> {
        let mut files = Vec::new();
        collect_projects(&self.workspace, &self.workspace, &mut files, 0);
        files.sort();
        Ok(ToolOutput::text(if files.is_empty() {
            "no .vfproj files in the workspace".to_string()
        } else {
            files.join("\n")
        })
        .with_structured(json!({ "projects": files })))
    }

    fn terrain_generate(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize, Default)]
        struct Args {
            mode: Option<String>,
            size: Option<u32>,
            seed: Option<u64>,
            preset: Option<String>,
            dimension: Option<f32>,
            divisor: Option<u8>,
            island: Option<bool>,
            vertical_scale: Option<f32>,
            erosion: Option<bool>,
            droplets: Option<u32>,
        }
        let args: Args = parse_args(args)?;
        let seed = args.seed.unwrap_or(1);
        let size = args.size.unwrap_or(1026).clamp(2, 4096);
        let mode = args.mode.unwrap_or_else(|| "classic".into());
        let preset = args.preset.unwrap_or_else(|| "alpine".into());
        let preview = self.last_preview.clone();
        let loaded = self.loaded_mut()?;
        let name = loaded.project.meta.name.clone();
        let (mut project, heightfield) =
            build_terrain_with(&name, seed, size, &mode, &preset, |source| match source {
                TerrainSource::Classic { params } => TerrainSource::Classic {
                    params: ClassicParams {
                        fractal_dimension: args.dimension.unwrap_or(params.fractal_dimension),
                        divisor: args.divisor.unwrap_or(params.divisor),
                        island: args.island.unwrap_or(params.island),
                        vertical_scale: args.vertical_scale.unwrap_or(params.vertical_scale),
                        ..params
                    },
                },
                TerrainSource::Modern { params } => TerrainSource::Modern {
                    params: ModernParams {
                        vertical_scale: args.vertical_scale.unwrap_or(params.vertical_scale),
                        ..params
                    },
                },
                other => other,
            })?;
        let heightfield = if args.erosion.unwrap_or(false) {
            let mut params = ErosionParams::scaled_for(size);
            if let Some(droplets) = args.droplets {
                params.droplets = droplets;
            }
            vf_procgen::erosion::erode(&heightfield, &params, Seed::new(seed).derive("erosion"))
                .map_err(|e| format!("erosion failed: {e}"))?
        } else {
            heightfield
        };
        project.sync_terrain(&heightfield);
        project.camera = auto_camera(&heightfield, &project);
        project.mark_modified();
        let path = loaded.path.clone();
        if let Some(path) = &path {
            vf_io::save_project(&project, &heightfield, path, preview.as_ref())
                .map_err(|e| format!("cannot save project: {e}"))?;
        }
        let stats = heightfield.stats();
        self.last_preview = None;
        self.loaded = Some(Loaded {
            project,
            heightfield,
            path,
        });
        Ok(ToolOutput::text(format!(
            "generated {mode} {size}x{size} seed {seed}\nrelief {:.1} m",
            stats.relief()
        ))
        .with_structured(json!({
            "mode": mode,
            "size": size,
            "seed": seed,
            "stats": stats,
        })))
    }

    fn terrain_import_dem(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize)]
        struct Args {
            path: String,
            spacing: Option<f32>,
            name: Option<String>,
        }
        let args: Args = parse_args(args)?;
        let path = self.existing_path(&args.path)?;
        let bytes =
            std::fs::read(&path).map_err(|e| format!("cannot read '{}': {e}", path.display()))?;
        let dem = DemFile::parse(&bytes).map_err(|e| format!("invalid DEM: {e}"))?;
        let spacing = args.spacing.unwrap_or(30.0);
        let heightfield = dem
            .to_heightfield(spacing)
            .map_err(|e| format!("cannot convert DEM: {e}"))?;
        let title = args.name.unwrap_or_else(|| dem.header.title.clone());
        let mut project = Project::new(
            title.clone(),
            Seed::new(fnv1a(&title)),
            heightfield.width() as u32,
        );
        project.source = TerrainSource::Dem {
            source: DemSource {
                path: path.display().to_string(),
                title: dem.header.title.clone(),
                comment: dem.header.comment.clone(),
                spacing_m: spacing,
            },
        };
        project.legacy = Some(LegacyMeta {
            dem_title: dem.header.title.clone(),
            dem_comment: dem.header.comment.clone(),
            session_flags: dem.header.flags,
            ..LegacyMeta::default()
        });
        project.water.enabled = false;
        project.sync_terrain(&heightfield);
        project.camera = auto_camera(&heightfield, &project);
        let size = (heightfield.width(), heightfield.height());
        self.loaded = Some(Loaded {
            project,
            heightfield,
            path: None,
        });
        self.last_preview = None;
        Ok(ToolOutput::text(format!(
            "imported {} as {}x{} at {:.1} m spacing (use project.save to persist)",
            path.display(),
            size.0,
            size.1,
            spacing
        )))
    }

    fn terrain_resample(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize)]
        struct Args {
            width: usize,
            height: Option<usize>,
            filter: Option<String>,
        }
        let args: Args = parse_args(args)?;
        let filter = args
            .filter
            .as_deref()
            .and_then(vf_terrain::ResampleFilter::parse)
            .unwrap_or(vf_terrain::ResampleFilter::Bicubic);
        let loaded = self.loaded_mut()?;
        let hf = vf_terrain::resample(
            &loaded.heightfield,
            args.width,
            args.height.unwrap_or(args.width),
            filter,
        )
        .map_err(|e| format!("resample failed: {e}"))?;
        loaded.heightfield = hf;
        loaded.project.sync_terrain(&loaded.heightfield);
        loaded.project.camera = auto_camera(&loaded.heightfield, &loaded.project);
        let dimensions = (loaded.heightfield.width(), loaded.heightfield.height());
        self.last_preview = None;
        Ok(ToolOutput::text(format!(
            "resampled to {}x{}",
            dimensions.0, dimensions.1
        )))
    }

    fn terrain_smooth(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize)]
        struct Args {
            radius: Option<usize>,
            iterations: Option<usize>,
        }
        let args: Args = parse_args(args)?;
        let loaded = self.loaded_mut()?;
        loaded.heightfield = vf_terrain::smooth(
            &loaded.heightfield,
            args.radius.unwrap_or(2),
            args.iterations.unwrap_or(1),
        );
        loaded.project.sync_terrain(&loaded.heightfield);
        self.last_preview = None;
        Ok(ToolOutput::text("terrain smoothed".to_string()))
    }

    fn terrain_height_query(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize)]
        struct Args {
            points: Vec<[f64; 2]>,
        }
        let args: Args = parse_args(args)?;
        if args.points.len() > 10_000 {
            return Err("at most 10,000 points per query".into());
        }
        let loaded = self.loaded_mut()?;
        let shaped = live_heightfield(loaded)?;
        let (ox, oy) = shaped.origin();
        let (sx, sy) = shaped.spacing();
        let mut results = Vec::with_capacity(args.points.len());
        for [x, y] in args.points {
            let gx = (x - ox) / sx;
            let gy = (y - oy) / sy;
            let inside = gx >= 0.0
                && gy >= 0.0
                && gx <= (shaped.width() - 1) as f64
                && gy <= (shaped.height() - 1) as f64;
            let z = if inside {
                Some(shaped.sample_bilinear(gx, gy))
            } else {
                None
            };
            results.push(json!({ "x": x, "y": y, "z": z, "inside": inside }));
        }
        let text = serde_json::to_string_pretty(&results).unwrap_or_default();
        Ok(ToolOutput::text(text).with_structured(json!({ "points": results })))
    }

    fn terrain_export_dem(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize)]
        struct Args {
            path: String,
            title: Option<String>,
            comment: Option<String>,
        }
        let args: Args = parse_args(args)?;
        let path = resolve_output(&self.workspace, &args.path)?;
        let loaded = self.loaded_mut()?;
        let shaped = live_heightfield(loaded)?;
        let dem = DemFile::from_heightfield(
            &shaped,
            args.title.as_deref().unwrap_or(&loaded.project.meta.name),
            args.comment.as_deref().unwrap_or("Exported by VistaForge"),
            Vec::new(),
        )
        .map_err(|e| format!("cannot build DEM: {e}"))?;
        let bytes = dem.write().map_err(|e| format!("cannot encode DEM: {e}"))?;
        std::fs::write(&path, bytes).map_err(|e| format!("cannot write DEM: {e}"))?;
        Ok(ToolOutput::text(format!("wrote {}", path.display()))
            .with_structured(json!({ "path": path })))
    }

    fn terrain_export_ascii(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize)]
        struct Args {
            path: String,
        }
        let args: Args = parse_args(args)?;
        let path = resolve_output(&self.workspace, &args.path)?;
        let loaded = self.loaded_mut()?;
        let shaped = live_heightfield(loaded)?;
        vf_io::ascii::write_ascii_grid(&path, &shaped, -9999.0)
            .map_err(|e| format!("cannot write ASCII grid: {e}"))?;
        Ok(ToolOutput::text(format!("wrote {}", path.display()))
            .with_structured(json!({ "path": path })))
    }

    fn scene_preset(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize)]
        struct Args {
            name: String,
            reframe: Option<bool>,
        }
        let args: Args = parse_args(args)?;
        let loaded = self.loaded_mut()?;
        loaded
            .project
            .apply_scene_preset(&args.name)
            .map_err(|e| e.to_string())?;
        if args.reframe.unwrap_or(true) {
            loaded.project.camera = auto_camera(&loaded.heightfield, &loaded.project);
        }
        self.last_preview = None;
        Ok(ToolOutput::text(format!(
            "scene preset '{}' applied",
            args.name
        )))
    }

    fn camera_set(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize, Default)]
        struct Args {
            position: Option<[f64; 3]>,
            target: Option<[f64; 3]>,
            fov: Option<f32>,
            auto_frame: Option<bool>,
            yaw: Option<f32>,
            pitch: Option<f32>,
            distance: Option<f64>,
        }
        let args: Args = parse_args(args)?;
        let loaded = self.loaded_mut()?;
        if args.auto_frame.unwrap_or(false) {
            loaded.project.camera = auto_camera(&loaded.heightfield, &loaded.project);
        } else {
            if let Some(p) = args.position {
                loaded.project.camera.position = glam::DVec3::from_array(p);
            }
            if let Some(t) = args.target {
                loaded.project.camera.target = glam::DVec3::from_array(t);
            }
            if args.yaw.is_some() || args.pitch.is_some() || args.distance.is_some() {
                let center = vf_scene::terrain_center(&loaded.heightfield);
                let distance = args
                    .distance
                    .unwrap_or_else(|| (loaded.heightfield.bounds().extent() * 1.4).max(100.0));
                loaded.project.camera = vf_scene::orbit(
                    center,
                    distance,
                    args.yaw.unwrap_or(35.0),
                    args.pitch.unwrap_or(30.0),
                    args.fov.unwrap_or(loaded.project.camera.fov_y_deg),
                );
            }
            if let Some(fov) = args.fov {
                loaded.project.camera.fov_y_deg = fov;
            }
        }
        loaded
            .project
            .camera
            .validate()
            .map_err(|e| format!("invalid camera: {e}"))?;
        let camera = json!({
            "position": loaded.project.camera.position.to_array(),
            "target": loaded.project.camera.target.to_array(),
            "fov": loaded.project.camera.fov_y_deg,
        });
        self.last_preview = None;
        Ok(ToolOutput::text("camera updated".to_string()).with_structured(camera))
    }

    fn sun_set(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize, Default)]
        struct Args {
            azimuth: Option<f32>,
            declination: Option<f32>,
            intensity: Option<f32>,
            color_k: Option<f32>,
            angular_radius: Option<f32>,
        }
        let args: Args = parse_args(args)?;
        let loaded = self.loaded_mut()?;
        let sun = &mut loaded.project.sun;
        if let Some(v) = args.azimuth {
            sun.azimuth_deg_from_south = v;
        }
        if let Some(v) = args.declination {
            sun.declination_deg = v;
        }
        if let Some(v) = args.intensity {
            sun.intensity = v;
        }
        if let Some(v) = args.color_k {
            sun.color_k = v;
        }
        if let Some(v) = args.angular_radius {
            sun.angular_radius_deg = v;
        }
        sun.validate().map_err(|e| format!("invalid sun: {e}"))?;
        let result = json!({
            "azimuth_from_south": sun.azimuth_deg_from_south,
            "declination": sun.declination_deg,
            "intensity": sun.intensity,
            "color_k": sun.color_k,
        });
        self.last_preview = None;
        Ok(ToolOutput::text(format!("sun updated: {result}")).with_structured(result))
    }

    fn water_set(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize, Default)]
        struct Args {
            enabled: Option<bool>,
            sea_level: Option<f32>,
            opacity: Option<f32>,
            wave_strength: Option<f32>,
            foam: Option<f32>,
        }
        let args: Args = parse_args(args)?;
        let loaded = self.loaded_mut()?;
        let water = &mut loaded.project.water;
        if let Some(v) = args.enabled {
            water.enabled = v;
        }
        if let Some(v) = args.sea_level {
            water.sea_level = v;
        }
        if let Some(v) = args.opacity {
            water.opacity = v;
        }
        if let Some(v) = args.wave_strength {
            water.wave_strength = v;
        }
        if let Some(v) = args.foam {
            water.foam = v;
        }
        water
            .validate()
            .map_err(|e| format!("invalid water: {e}"))?;
        let result = json!({
            "enabled": water.enabled,
            "sea_level": water.sea_level,
            "opacity": water.opacity,
        });
        self.last_preview = None;
        Ok(ToolOutput::text(format!("water updated: {result}")).with_structured(result))
    }

    fn atmosphere_set(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize, Default)]
        struct Args {
            enabled: Option<bool>,
            haze_density: Option<f32>,
            ambient_strength: Option<f32>,
            sky_intensity: Option<f32>,
            exposure: Option<f32>,
            contrast: Option<f32>,
            saturation: Option<f32>,
        }
        let args: Args = parse_args(args)?;
        let loaded = self.loaded_mut()?;
        let atmosphere = &mut loaded.project.atmosphere;
        if let Some(v) = args.enabled {
            atmosphere.enabled = v;
        }
        if let Some(v) = args.haze_density {
            atmosphere.haze_density = v;
        }
        if let Some(v) = args.ambient_strength {
            atmosphere.ambient_strength = v;
        }
        if let Some(v) = args.sky_intensity {
            atmosphere.sky_intensity = v;
        }
        let render = &mut loaded.project.render;
        if let Some(v) = args.exposure {
            render.exposure = v;
        }
        if let Some(v) = args.contrast {
            render.contrast = v;
        }
        if let Some(v) = args.saturation {
            render.saturation = v;
        }
        atmosphere
            .validate()
            .map_err(|e| format!("invalid atmosphere: {e}"))?;
        render
            .validate()
            .map_err(|e| format!("invalid render settings: {e}"))?;
        self.last_preview = None;
        Ok(ToolOutput::text("atmosphere updated".to_string()))
    }

    fn modifiers_kinds(&mut self) -> std::result::Result<ToolOutput, String> {
        let kinds: Vec<Value> = ModifierKind::ALL
            .iter()
            .map(|kind| {
                let params: Vec<Value> = kind
                    .specs()
                    .iter()
                    .map(|spec| {
                        json!({
                            "key": spec.key,
                            "label": spec.label,
                            "unit": spec.unit,
                            "min": spec.min,
                            "max": spec.max,
                            "default": spec.default,
                            "integer": spec.integer,
                            "logarithmic": spec.logarithmic,
                            "choices": spec.choices,
                        })
                    })
                    .collect();
                json!({
                    "kind": kind.name(),
                    "label": kind.label(),
                    "group": kind.group(),
                    "params": params,
                })
            })
            .collect();
        Ok(ToolOutput::text(format!("{} modifier kinds", kinds.len()))
            .with_structured(json!({ "kinds": kinds })))
    }

    fn modifiers_list(&mut self) -> std::result::Result<ToolOutput, String> {
        let loaded = self.loaded_mut()?;
        let items: Vec<Value> = loaded
            .project
            .modifiers
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| modifier_json(index, item))
            .collect();
        let summary = loaded.project.modifiers.summary();
        Ok(ToolOutput::text(format!(
            "modifier stack ({} entries): {summary}",
            items.len()
        ))
        .with_structured(json!({ "items": items, "summary": summary })))
    }

    fn modifiers_add(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize)]
        struct Args {
            kind: String,
            params: Option<std::collections::BTreeMap<String, f32>>,
        }
        let args: Args = parse_args(args)?;
        let kind = ModifierKind::parse(&args.kind)
            .ok_or_else(|| format!("unknown modifier '{}'; use modifiers.kinds", args.kind))?;
        let (summary, structured) = {
            let loaded = self.loaded_mut()?;
            let index = loaded.project.modifiers.push(kind);
            let item = &mut loaded.project.modifiers.items[index];
            if let Some(params) = args.params {
                for (key, value) in params {
                    if !item.set(&key, value) {
                        return Err(format!("{} has no parameter '{key}'", item.kind.name()));
                    }
                }
            }
            loaded.project.modifiers.normalize();
            loaded
                .project
                .modifiers
                .validate()
                .map_err(|e| e.to_string())?;
            let item = &loaded.project.modifiers.items[index];
            (
                format!("added modifier {index}: {}", item.summary()),
                modifier_json(index, item),
            )
        };
        self.last_preview = None;
        Ok(ToolOutput::text(summary).with_structured(structured))
    }

    fn modifiers_set(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize)]
        struct Args {
            index: usize,
            params: Option<std::collections::BTreeMap<String, f32>>,
            enabled: Option<bool>,
        }
        let args: Args = parse_args(args)?;
        let (summary, structured) = {
            let loaded = self.loaded_mut()?;
            let item = loaded
                .project
                .modifiers
                .items
                .get_mut(args.index)
                .ok_or_else(|| format!("modifier {} does not exist", args.index))?;
            if let Some(enabled) = args.enabled {
                item.enabled = enabled;
            }
            if let Some(params) = args.params {
                for (key, value) in params {
                    if !item.set(&key, value) {
                        return Err(format!("{} has no parameter '{key}'", item.kind.name()));
                    }
                }
            }
            loaded.project.modifiers.normalize();
            loaded
                .project
                .modifiers
                .validate()
                .map_err(|e| e.to_string())?;
            let item = &loaded.project.modifiers.items[args.index];
            (
                format!("modified modifier {}: {}", args.index, item.summary()),
                modifier_json(args.index, item),
            )
        };
        self.last_preview = None;
        Ok(ToolOutput::text(summary).with_structured(structured))
    }

    fn modifiers_remove(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize)]
        struct Args {
            index: usize,
        }
        let args: Args = parse_args(args)?;
        let summary = {
            let loaded = self.loaded_mut()?;
            let removed = loaded
                .project
                .modifiers
                .remove(args.index)
                .ok_or_else(|| format!("modifier {} does not exist", args.index))?;
            format!("removed modifier {}: {}", args.index, removed.summary())
        };
        self.last_preview = None;
        Ok(ToolOutput::text(summary))
    }

    fn modifiers_clear(&mut self) -> std::result::Result<ToolOutput, String> {
        let count = {
            let loaded = self.loaded_mut()?;
            let count = loaded.project.modifiers.items.len();
            loaded.project.modifiers.clear();
            count
        };
        self.last_preview = None;
        Ok(ToolOutput::text(format!("cleared {count} modifiers")))
    }

    fn modifiers_bake(&mut self) -> std::result::Result<ToolOutput, String> {
        let (before, after) = {
            let loaded = self.loaded_mut()?;
            if loaded.project.modifiers.is_empty() {
                return Ok(ToolOutput::text("modifier stack is empty".into()));
            }
            let before = loaded.heightfield.stats();
            let shaped = vf_modifiers::apply(
                &loaded.heightfield,
                &loaded.project.modifiers,
                loaded.project.seed,
            )
            .map_err(|e| format!("modifier failed: {e}"))?;
            loaded.project.modifiers.clear();
            loaded.heightfield = shaped;
            loaded.project.sync_terrain(&loaded.heightfield);
            (before.relief(), loaded.heightfield.stats().relief())
        };
        self.last_preview = None;
        Ok(ToolOutput::text(format!(
            "baked modifiers: relief {before:.1} -> {after:.1} m"
        ))
        .with_structured(json!({
            "relief_before": before,
            "relief_after": after,
        })))
    }

    fn geology_set(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize, Default)]
        struct Args {
            preset: Option<String>,
            enabled: Option<bool>,
            auto_palette: Option<bool>,
            regenerate: Option<bool>,
        }
        let args: Args = parse_args(args)?;
        let preview = self.last_preview.clone();
        let loaded = self.loaded_mut()?;
        let mut settings = match &args.preset {
            Some(name) => GeologySettings::preset(name)
                .ok_or_else(|| format!("unknown rock preset '{name}'"))?,
            None => loaded.project.geology.clone(),
        };
        if let Some(enabled) = args.enabled {
            settings.enabled = enabled;
        }
        if let Some(auto_palette) = args.auto_palette {
            settings.auto_palette = auto_palette;
        }
        settings
            .validate()
            .map_err(|e| format!("invalid geology: {e}"))?;
        loaded.project.geology = settings.clone();
        if let Some(name) = &args.preset
            && let Some(detail) = vf_core::DetailSettings::preset(name)
        {
            loaded.project.detail = detail;
        }
        if let Some(enabled) = args.enabled {
            loaded.project.detail.enabled = enabled;
        }
        if settings.auto_palette
            && let Some(palette) = vf_core::Palette::preset(settings.suggested_palette())
        {
            loaded.project.palette = palette;
        }
        let regenerate = args.regenerate.unwrap_or(true);
        if regenerate {
            let seed = loaded.project.seed;
            let erosion = if loaded.project.erosion_enabled {
                Some(loaded.project.erosion.clone())
            } else {
                None
            };
            let heightfield =
                vf_geology::build(&loaded.project.source, erosion.as_ref(), &settings, seed)
                    .map_err(|e| format!("geology failed: {e}"))?;
            loaded.project.sync_terrain(&heightfield);
            loaded.project.camera = auto_camera(&heightfield, &loaded.project);
            loaded.heightfield = heightfield;
        }
        loaded.project.mark_modified();
        let path = loaded.path.clone();
        let (project, heightfield) = (&loaded.project, &loaded.heightfield);
        if let Some(path) = &path {
            vf_io::save_project(project, heightfield, path, preview.as_ref())
                .map_err(|e| format!("cannot save project: {e}"))?;
        }
        let stats = heightfield.stats();
        self.last_preview = None;
        Ok(ToolOutput::text(format!(
            "rock preset '{}' enabled={} relief {:.1} m",
            settings.preset,
            settings.enabled,
            stats.relief()
        ))
        .with_structured(json!({
            "preset": settings.preset,
            "enabled": settings.enabled,
            "regenerated": regenerate,
            "relief_m": stats.relief(),
        })))
    }

    fn flight_settings(
        preset: Option<&str>,
        project: &Project,
    ) -> std::result::Result<FlightSettings, String> {
        let settings = match preset {
            Some(name) => FlightSettings::preset(name)
                .ok_or_else(|| format!("unknown flight preset '{name}'"))?,
            None if project.flight.enabled => project.flight.clone(),
            None => FlightSettings::preset("cinelifter").expect("built-in preset"),
        };
        settings
            .validate()
            .map_err(|e| format!("invalid flight: {e}"))?;
        Ok(settings)
    }

    fn flight_info(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize, Default)]
        struct Args {
            preset: Option<String>,
            seconds: Option<f32>,
            fps: Option<u32>,
        }
        let args: Args = parse_args(args)?;
        let loaded = self.loaded_mut()?;
        let mut settings = Self::flight_settings(args.preset.as_deref(), &loaded.project)?;
        if let Some(seconds) = args.seconds {
            settings.duration_s = seconds;
        }
        if let Some(fps) = args.fps {
            settings.fps = fps;
        }
        let shaped = live_heightfield(loaded)?;
        let samples = vf_flight::simulate(
            &settings,
            &shaped,
            Seed::new(settings.seed),
            loaded.project.camera.fov_y_deg,
        )
        .map_err(|e| format!("flight failed: {e}"))?;
        let max_bank = samples
            .iter()
            .map(|s| s.bank_deg.abs())
            .fold(0.0f64, f64::max);
        let min_agl = samples
            .iter()
            .map(|s| s.altitude_agl_m)
            .fold(f64::INFINITY, f64::min);
        Ok(ToolOutput::text(format!(
            "flight '{}': {} frames, {:.1}s, max bank {:.1}°, min AGL {:.1} m",
            settings.preset,
            samples.len(),
            settings.duration_s,
            max_bank,
            min_agl
        ))
        .with_structured(json!({
            "preset": settings.preset,
            "frames": samples.len(),
            "duration_s": settings.duration_s,
            "fps": settings.fps,
            "max_bank_deg": max_bank,
            "min_agl_m": min_agl,
        })))
    }

    fn flight_still(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        #[derive(Deserialize, Default)]
        struct Args {
            preset: Option<String>,
            time: Option<f64>,
            output: String,
            width: Option<u32>,
            height: Option<u32>,
        }
        let args: Args = parse_args(args)?;
        if args.output.is_empty() {
            return Err("flight.still requires an output path".into());
        }
        let workspace = self.workspace.clone();
        let path = resolve_output(&workspace, &args.output)?;
        let loaded = self.loaded_mut()?;
        let settings = Self::flight_settings(args.preset.as_deref(), &loaded.project)?;
        let time = args.time.unwrap_or(0.0).max(0.0);
        let shaped = live_heightfield(loaded)?;
        let sample = vf_flight::sample_at(
            &settings,
            &shaped,
            Seed::new(settings.seed),
            loaded.project.camera.fov_y_deg,
            time,
        )
        .map_err(|e| format!("flight failed: {e}"))?;
        let mut project = loaded.project.clone();
        project.camera = sample.camera;
        let mut render = project.render.clone();
        if let Some(width) = args.width {
            render.width = width;
        }
        if let Some(height) = args.height {
            render.height = height;
        }
        let options = RenderOptions::from(&render);
        let output = render_with_options(&project, &loaded.heightfield, &options)
            .map_err(|e| format!("render failed: {e}"))?;
        output
            .save_png(&path)
            .map_err(|e| format!("cannot write PNG: {e}"))?;
        self.last_preview = Some(output.image.clone());
        let stats = json!({
            "path": path,
            "time_s": time,
            "preset": settings.preset,
            "width": output.stats.width,
            "height": output.stats.height,
            "altitude_agl_m": sample.altitude_agl_m,
            "bank_deg": sample.bank_deg,
            "elapsed_ms": output.stats.elapsed_ms,
        });
        Ok(ToolOutput::text(format!(
            "flight still '{}' t={:.2}s agl {:.1} m -> {}",
            settings.preset,
            time,
            sample.altitude_agl_m,
            path.display()
        ))
        .with_structured(stats))
    }

    fn render_still(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        let args: RenderArgs = parse_args(args)?;
        if args.output.is_empty() {
            return Err("render.still requires an output path".into());
        }
        let workspace = self.workspace.clone();
        let path = resolve_output(&workspace, &args.output)?;
        let loaded = self.loaded_mut()?;
        let options = render_options(&loaded.project, &args)?;
        let output = render_with_options(&loaded.project, &loaded.heightfield, &options)
            .map_err(|e| format!("render failed: {e}"))?;
        output
            .save_png(&path)
            .map_err(|e| format!("cannot write PNG: {e}"))?;
        self.last_preview = Some(output.image.clone());
        let stats = json!({
            "path": path,
            "width": output.stats.width,
            "height": output.stats.height,
            "ssaa": output.stats.ssaa,
            "triangles": output.stats.triangles,
            "elapsed_ms": output.stats.elapsed_ms,
        });
        Ok(ToolOutput::text(format!(
            "rendered {}x{} in {} ms -> {}",
            output.stats.width,
            output.stats.height,
            output.stats.elapsed_ms,
            path.display()
        ))
        .with_structured(stats))
    }

    fn render_preview(&mut self, args: Value) -> std::result::Result<ToolOutput, String> {
        let args: RenderArgs = parse_args(args)?;
        let mut preview_args = args;
        preview_args.width = Some(preview_args.width.unwrap_or(960).min(1280));
        preview_args.height = Some(preview_args.height.unwrap_or(540).min(720));
        preview_args.preview = true;
        let loaded = self.loaded_mut()?;
        let options = render_options(&loaded.project, &preview_args)?;
        let output = render_with_options(&loaded.project, &loaded.heightfield, &options)
            .map_err(|e| format!("render failed: {e}"))?;
        let mut png = Vec::new();
        {
            let buffer = image::RgbaImage::from_raw(
                output.image.width,
                output.image.height,
                output.image.pixels.clone(),
            )
            .ok_or_else(|| "invalid preview buffer".to_string())?;
            image::DynamicImage::ImageRgba8(buffer)
                .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                .map_err(|e| format!("PNG encode failed: {e}"))?;
        }
        self.last_preview = Some(output.image.clone());
        let stats = json!({
            "width": output.stats.width,
            "height": output.stats.height,
            "elapsed_ms": output.stats.elapsed_ms,
        });
        let mut tool_output = ToolOutput::text(format!(
            "preview {}x{} in {} ms",
            output.stats.width, output.stats.height, output.stats.elapsed_ms
        ))
        .with_structured(stats);
        tool_output.image = Some(png);
        Ok(tool_output)
    }
}

struct ToolOutput {
    text: String,
    structured: Option<Value>,
    image: Option<Vec<u8>>,
}

impl ToolOutput {
    fn text(text: String) -> Self {
        Self {
            text,
            structured: None,
            image: None,
        }
    }

    fn with_structured(mut self, structured: Value) -> Self {
        self.structured = Some(structured);
        self
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RenderArgs {
    output: String,
    width: Option<u32>,
    height: Option<u32>,
    quality: Option<String>,
    ssaa: Option<u8>,
    shadows: Option<bool>,
    ao: Option<bool>,
    cliffs: Option<bool>,
    clouds: Option<bool>,
    water: Option<bool>,
    exposure: Option<f32>,
    preview: bool,
}

fn render_options(
    project: &Project,
    args: &RenderArgs,
) -> std::result::Result<RenderOptions, String> {
    let mut settings = project.render.clone();
    if let Some(quality) = &args.quality {
        let quality = match quality.to_ascii_lowercase().as_str() {
            "low" => Quality::Low,
            "medium" => Quality::Medium,
            "high" => Quality::High,
            "ultra" => Quality::Ultra,
            other => return Err(format!("unknown quality '{other}'")),
        };
        settings.apply_quality(quality);
    }
    if let Some(width) = args.width {
        settings.width = width;
    }
    if let Some(height) = args.height {
        settings.height = height;
    }
    if let Some(ssaa) = args.ssaa {
        settings.ssaa = ssaa;
    }
    if let Some(shadows) = args.shadows {
        settings.shadows = shadows;
    }
    if let Some(ao) = args.ao {
        settings.ao = ao;
    }
    if let Some(cliffs) = args.cliffs {
        settings.cliffs = cliffs;
    }
    if let Some(clouds) = args.clouds {
        settings.clouds = clouds;
    }
    if let Some(water) = args.water {
        settings.water = water;
    }
    if let Some(exposure) = args.exposure {
        settings.exposure = exposure;
    }
    let mut options = RenderOptions::from(&settings);
    if args.preview {
        options.ssaa = 1;
        options.shadows = false;
        options.ao = false;
    }
    options.validate().map_err(|e| e.to_string())?;
    Ok(options)
}

fn build_terrain(
    name: &str,
    seed: u64,
    size: u32,
    mode: &str,
    preset: &str,
    erosion: bool,
) -> std::result::Result<(Project, HeightField), String> {
    let (project, mut heightfield) =
        build_terrain_with(name, seed, size, mode, preset, |source| source)?;
    if erosion {
        let params = ErosionParams::scaled_for(size);
        heightfield =
            vf_procgen::erosion::erode(&heightfield, &params, Seed::new(seed).derive("erosion"))
                .map_err(|e| format!("erosion failed: {e}"))?;
    }
    Ok((project, heightfield))
}

fn build_terrain_with<F>(
    name: &str,
    seed: u64,
    size: u32,
    mode: &str,
    preset: &str,
    adjust: F,
) -> std::result::Result<(Project, HeightField), String>
where
    F: FnOnce(TerrainSource) -> TerrainSource,
{
    let mut project = Project::new(name.to_string(), Seed::new(seed), size);
    let source = match mode.to_ascii_lowercase().as_str() {
        "classic" => TerrainSource::Classic {
            params: ClassicParams::default().with_size_seed(size, seed),
        },
        "modern" => {
            let params = vf_core::terrain_params::modern_preset(preset)
                .ok_or_else(|| format!("unknown modern preset '{preset}'"))?
                .with_size_seed(size, seed);
            TerrainSource::Modern { params }
        }
        other => return Err(format!("unknown terrain mode '{other}'")),
    };
    project.source = adjust(source);
    let heightfield = vf_procgen::generate(&project.source, None, Seed::new(seed))
        .map_err(|e| format!("generation failed: {e}"))?;
    project.sync_terrain(&heightfield);
    project.camera = auto_camera(&heightfield, &project);
    Ok((project, heightfield))
}

fn auto_camera(hf: &HeightField, project: &Project) -> vf_core::Camera {
    let aspect = f64::from(project.render.width.max(1)) / f64::from(project.render.height.max(1));
    vf_scene::auto_frame(hf, aspect)
}

fn parse_args<T: DeserializeOwned>(value: Value) -> std::result::Result<T, String> {
    serde_json::from_value(value).map_err(|e| format!("invalid arguments: {e}"))
}

fn resolve_output(workspace: &Path, raw: &str) -> std::result::Result<PathBuf, String> {
    let path = Path::new(raw);
    if path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("output paths must stay inside the workspace".into());
    }
    let path = workspace.join(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create directory: {e}"))?;
    }
    Ok(path)
}

fn collect_projects(root: &Path, dir: &Path, out: &mut Vec<String>, depth: usize) {
    if depth > 4 || out.len() > 200 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| {
                n == std::ffi::OsStr::new("target") || n == std::ffi::OsStr::new(".git")
            }) {
                continue;
            }
            collect_projects(root, &path, out, depth + 1);
        } else if path.extension().is_some_and(|e| e == "vfproj")
            && let Ok(relative) = path.strip_prefix(root)
        {
            out.push(relative.display().to_string());
        }
    }
}

fn slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "landscape".into()
    } else {
        trimmed
    }
}

fn fnv1a(text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// Serve MCP over newline-delimited JSON. Returns on EOF.
pub fn serve<R: BufRead, W: Write>(
    mut input: R,
    mut output: W,
    workspace: PathBuf,
) -> std::io::Result<()> {
    let workspace = workspace.canonicalize().unwrap_or(workspace);
    let mut server = Server::new(workspace);
    let mut line = String::new();
    loop {
        line.clear();
        let read = input.read_line(&mut line)?;
        if read == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(trimmed) {
            Ok(message) => server.handle(message),
            Err(error) => Some(json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": { "code": -32700, "message": format!("parse error: {error}") }
            })),
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut output, &response)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    Ok(())
}

fn modifier_json(index: usize, item: &Modifier) -> Value {
    let params: Vec<Value> = item
        .specs()
        .iter()
        .map(|spec| {
            json!({
                "key": spec.key,
                "label": spec.label,
                "unit": spec.unit,
                "value": item.value(spec.key),
                "min": spec.min,
                "max": spec.max,
                "default": spec.default,
                "integer": spec.integer,
                "logarithmic": spec.logarithmic,
                "choices": spec.choices,
            })
        })
        .collect();
    json!({
        "index": index,
        "kind": item.kind.name(),
        "label": item.kind.label(),
        "group": item.kind.group(),
        "enabled": item.enabled,
        "summary": item.summary(),
        "params": params,
    })
}

fn tool_defs() -> Value {
    json!([
        tool(
            "project.new",
            "Create and save a new landscape project.",
            obj(
                json!({
                    "path": prop("string", "Workspace-relative .vfproj path (optional)."),
                    "name": prop("string", "Display name."),
                    "seed": prop("integer", "Deterministic seed (default 1)."),
                    "size": prop("integer", "Grid size (default 1026, max 4096)."),
                    "mode": prop("string", "classic or modern."),
                    "preset": prop("string", "Modern preset name."),
                    "erosion": prop("boolean", "Apply hydraulic and thermal erosion.")
                }),
                &[]
            )
        ),
        tool(
            "project.open",
            "Open an existing .vfproj inside the workspace.",
            obj(
                json!({ "path": prop("string", "Workspace-relative project path.") }),
                &["path"]
            )
        ),
        tool(
            "project.save",
            "Save the current project, optionally to a new workspace path.",
            obj(json!({ "path": prop("string", "Optional new path.") }), &[])
        ),
        tool(
            "project.info",
            "Return project metadata, terrain statistics, camera and sun.",
            obj(json!({}), &[])
        ),
        tool(
            "project.list",
            "List .vfproj files in the workspace.",
            obj(json!({}), &[])
        ),
        tool(
            "terrain.generate",
            "Regenerate the current project's terrain.",
            obj(
                json!({
                    "mode": prop("string", "classic or modern."),
                    "size": prop("integer", "Grid size."),
                    "seed": prop("integer", "Deterministic seed."),
                    "preset": prop("string", "Modern preset."),
                    "dimension": prop("number", "Classic FrDim 0..200."),
                    "divisor": prop("integer", "Classic divisor 1, 2, 4 or 8."),
                    "island": prop("boolean", "Classic island mode."),
                    "vertical_scale": prop("number", "Relief in metres."),
                    "erosion": prop("boolean", "Apply erosion."),
                    "droplets": prop("integer", "Erosion droplet count.")
                }),
                &[]
            )
        ),
        tool(
            "terrain.import_dem",
            "Import a legacy DEM from the workspace into memory.",
            obj(
                json!({
                    "path": prop("string", "Workspace-relative DEM path."),
                    "spacing": prop("number", "Sample spacing in metres (default 30)."),
                    "name": prop("string", "Project display name.")
                }),
                &["path"]
            )
        ),
        tool(
            "terrain.resample",
            "Resample the terrain to a new grid size.",
            obj(
                json!({
                    "width": prop("integer", "Target width."),
                    "height": prop("integer", "Target height (defaults to width)."),
                    "filter": prop("string", "nearest, bilinear or bicubic.")
                }),
                &["width"]
            )
        ),
        tool(
            "terrain.smooth",
            "Gaussian-smooth the terrain.",
            obj(
                json!({
                    "radius": prop("integer", "Kernel radius."),
                    "iterations": prop("integer", "Pass count.")
                }),
                &[]
            )
        ),
        tool(
            "terrain.height_query",
            "Sample terrain heights at world X/Y points.",
            obj(
                json!({
                    "points": json!({
                        "type": "array",
                        "items": json!({"type":"array","items":{"type":"number"},"minItems":2,"maxItems":2}),
                        "maxItems": 10000
                    })
                }),
                &["points"]
            )
        ),
        tool(
            "terrain.export_dem",
            "Export the current terrain as a legacy DEM.",
            obj(
                json!({
                    "path": prop("string", "Workspace-relative output path."),
                    "title": prop("string", "DEM title."),
                    "comment": prop("string", "DEM comment.")
                }),
                &["path"]
            )
        ),
        tool(
            "terrain.export_ascii",
            "Export the current terrain as an ESRI ASCII Grid.",
            obj(json!({ "path": prop("string", "Output path.") }), &["path"])
        ),
        tool(
            "geology.set",
            "Apply a rock family (strata, joints, karst) and rebuild the terrain.",
            obj(
                json!({
                    "preset": prop("string", "granite, basalt, limestone, sandstone, schist, chalk or volcanic."),
                    "enabled": prop("boolean", "Enable the geological model."),
                    "auto_palette": prop("boolean", "Pick a palette that matches the rock."),
                    "regenerate": prop("boolean", "Rebuild the heightfield (default true).")
                }),
                &[]
            )
        ),
        tool(
            "flight.info",
            "Simulate a flight and return frame count, bank and clearance statistics.",
            obj(
                json!({
                    "preset": prop("string", "cinelifter, freestyle, racing, helicopter, cinematic or dive."),
                    "seconds": prop("number", "Clip length in seconds."),
                    "fps": prop("integer", "Frames per second.")
                }),
                &[]
            )
        ),
        tool(
            "flight.still",
            "Render one frame of the simulated flight at a given time.",
            obj(
                json!({
                    "preset": prop("string", "Flight preset name."),
                    "time": prop("number", "Clip time in seconds."),
                    "output": prop("string", "Workspace-relative PNG path."),
                    "width": prop("integer", "Output width."),
                    "height": prop("integer", "Output height.")
                }),
                &["output"]
            )
        ),
        tool(
            "scene.preset",
            "Apply a lighting/palette preset.",
            obj(
                json!({
                    "name": prop("string", "alpine, sunset, desert, volcanic, lunar or coastal."),
                    "reframe": prop("boolean", "Reframe the camera (default true).")
                }),
                &["name"]
            )
        ),
        tool(
            "camera.set",
            "Set the camera position/target/FOV, orbit it, or auto-frame the terrain.",
            obj(
                json!({
                    "position": array3("World position [x,y,z]."),
                    "target": array3("World target [x,y,z]."),
                    "fov": prop("number", "Vertical FOV in degrees."),
                    "auto_frame": prop("boolean", "Fit the whole terrain."),
                    "yaw": prop("number", "Orbit yaw: 0 = south of target."),
                    "pitch": prop("number", "Orbit elevation in degrees."),
                    "distance": prop("number", "Orbit distance in metres.")
                }),
                &[]
            )
        ),
        tool(
            "sun.set",
            "Set sun azimuth (0 = south, 90 = west), declination and colour.",
            obj(
                json!({
                    "azimuth": prop("number", "Compass azimuth in degrees from south."),
                    "declination": prop("number", "Elevation above horizon, 0..90."),
                    "intensity": prop("number", "Sun intensity."),
                    "color_k": prop("number", "Colour temperature in kelvin."),
                    "angular_radius": prop("number", "Angular radius in degrees.")
                }),
                &[]
            )
        ),
        tool(
            "water.set",
            "Enable or tune the global water plane.",
            obj(
                json!({
                    "enabled": prop("boolean", "Water on/off."),
                    "sea_level": prop("number", "Water altitude in metres."),
                    "opacity": prop("number", "0..1."),
                    "wave_strength": prop("number", "Wave normal strength."),
                    "foam": prop("number", "Shore foam amount 0..1.")
                }),
                &[]
            )
        ),
        tool(
            "atmosphere.set",
            "Tune haze, ambient light and colour grading.",
            obj(
                json!({
                    "enabled": prop("boolean", "Haze on/off."),
                    "haze_density": prop("number", "Haze density per metre."),
                    "ambient_strength": prop("number", "Sky ambient 0..1."),
                    "sky_intensity": prop("number", "Sky brightness."),
                    "exposure": prop("number", "Render exposure."),
                    "contrast": prop("number", "Post contrast."),
                    "saturation": prop("number", "Post saturation.")
                }),
                &[]
            )
        ),
        tool(
            "modifiers.kinds",
            "List every modifier kind with its editable parameters and ranges.",
            obj(json!({}), &[])
        ),
        tool(
            "modifiers.list",
            "Return the current modifier stack, in application order.",
            obj(json!({}), &[])
        ),
        tool(
            "modifiers.add",
            "Append a modifier to the stack. Values not given keep their defaults.",
            obj(
                json!({
                    "kind": prop("string", "Modifier name; see modifiers.kinds."),
                    "params": params_schema(),
                }),
                &["kind"]
            )
        ),
        tool(
            "modifiers.set",
            "Change parameters or enable/disable one stack entry.",
            obj(
                json!({
                    "index": prop("integer", "Stack index from modifiers.list."),
                    "params": params_schema(),
                    "enabled": prop("boolean", "Enable or disable the entry.")
                }),
                &["index"]
            )
        ),
        tool(
            "modifiers.remove",
            "Remove one entry from the stack.",
            obj(
                json!({ "index": prop("integer", "Stack index from modifiers.list.") }),
                &["index"]
            )
        ),
        tool(
            "modifiers.clear",
            "Remove every modifier from the stack.",
            obj(json!({}), &[])
        ),
        tool(
            "modifiers.bake",
            "Apply the stack to the stored heightfield and clear it.",
            obj(json!({}), &[])
        ),
        tool(
            "render.still",
            "Render the current project to a PNG in the workspace.",
            render_schema(true)
        ),
        tool(
            "render.preview",
            "Render a fast preview and return it as an MCP image.",
            render_schema(false)
        )
    ])
}

fn params_schema() -> Value {
    json!({
        "type": "object",
        "description": "Parameter overrides by key (numbers). Use modifiers.kinds for keys and ranges.",
        "additionalProperties": { "type": "number" }
    })
}

fn render_schema(require_output: bool) -> Value {
    let mut properties = json!({
        "output": prop("string", "Workspace-relative PNG path."),
        "width": prop("integer", "Image width."),
        "height": prop("integer", "Image height."),
        "quality": prop("string", "low, medium, high or ultra."),
        "ssaa": prop("integer", "Supersampling 1..3."),
        "shadows": prop("boolean", "Sun shadows."),
        "ao": prop("boolean", "Ambient occlusion."),
        "cliffs": prop("boolean", "Local cliff shaping on steep faces."),
        "clouds": prop("boolean", "Cloud layer."),
        "water": prop("boolean", "Water plane."),
        "exposure": prop("number", "Exposure override.")
    });
    if !require_output {
        properties
            .as_object_mut()
            .expect("schema object")
            .remove("output");
    }
    obj(properties, if require_output { &["output"] } else { &[] })
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}

fn obj(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn prop(kind: &str, description: &str) -> Value {
    json!({ "type": kind, "description": description })
}

fn array3(description: &str) -> Value {
    json!({
        "type": "array",
        "description": description,
        "items": { "type": "number" },
        "minItems": 3,
        "maxItems": 3
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn run_session(messages: &[Value], workspace: &Path) -> Vec<Value> {
        let mut input = String::new();
        for message in messages {
            input.push_str(&serde_json::to_string(message).unwrap());
            input.push('\n');
        }
        let mut output = Vec::new();
        serve(
            Cursor::new(input.into_bytes()),
            &mut output,
            workspace.to_path_buf(),
        )
        .unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn geology_and_flight_tools() {
        let workspace = std::env::temp_dir().join(format!("vf-mcp-geo-{}", std::process::id()));
        std::fs::create_dir_all(&workspace).unwrap();
        let responses = run_session(
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                    "name":"project.new",
                    "arguments":{"path":"geo.vfproj","name":"Geo","seed":3,"size":33,"mode":"modern"}
                }}),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                    "name":"geology.set","arguments":{"preset":"sandstone","regenerate":true}
                }}),
                json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
                    "name":"flight.info","arguments":{"preset":"cinematic","seconds":1.0,"fps":4}
                }}),
            ],
            &workspace,
        );
        for response in &responses {
            assert_ne!(
                response["result"]["isError"], true,
                "tool failed: {response}"
            );
        }
        assert_eq!(
            responses[1]["result"]["structuredContent"]["preset"],
            "sandstone"
        );
        assert_eq!(responses[2]["result"]["structuredContent"]["frames"], 4);
        std::fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn protocol_round_trip() {
        let workspace = std::env::temp_dir().join(format!("vf-mcp-test-{}", std::process::id()));
        std::fs::create_dir_all(&workspace).unwrap();
        let responses = run_session(
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
                json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
                    "name":"project.new",
                    "arguments":{"path":"scene.vfproj","name":"MCP test","seed":7,"size":33,"mode":"classic"}
                }}),
                json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
                    "name":"project.info","arguments":{}
                }}),
                json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{
                    "name":"terrain.height_query","arguments":{"points":[[0.0,0.0],[100.0,100.0]]}
                }}),
                json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{
                    "name":"render.preview","arguments":{"width":32,"height":24}
                }}),
                json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{
                    "name":"project.save","arguments":{}
                }}),
                json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{
                    "name":"project.new","arguments":{"path":"../escape.vfproj"}
                }}),
            ],
            &workspace,
        );

        assert_eq!(responses.len(), 8, "every request must get a response");
        assert_eq!(responses[0]["result"]["protocolVersion"], PROTOCOL_VERSION);
        let tools = responses[1]["result"]["tools"].as_array().unwrap();
        assert!(tools.len() >= 15);
        assert!(tools.iter().any(|t| t["name"] == "render.still"));
        for index in [2usize, 3, 4, 5, 6] {
            assert_eq!(
                responses[index]["result"]["isError"], false,
                "tool {} failed: {}",
                responses[index]["id"], responses[index]["result"]
            );
        }
        let content = responses[5]["result"]["content"].as_array().unwrap();
        assert!(content.iter().any(|c| c["type"] == "image"));
        assert_eq!(responses[7]["result"]["isError"], true);
        assert!(workspace.join("scene.vfproj").exists());
        std::fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn modifier_tools_round_trip() {
        let workspace = std::env::temp_dir().join(format!("vf-mcp-mod-{}", std::process::id()));
        std::fs::create_dir_all(&workspace).unwrap();
        let responses = run_session(
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                    "name":"project.new",
                    "arguments":{"path":"mod.vfproj","name":"Mod","seed":5,"size":33,"mode":"classic"}
                }}),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                    "name":"modifiers.kinds","arguments":{}
                }}),
                json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
                    "name":"modifiers.add",
                    "arguments":{"kind":"displace","params":{"amount_m":80.0,"scale_m":600.0}}
                }}),
                json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
                    "name":"modifiers.add","arguments":{"kind":"crater","params":{"count":2.0}}
                }}),
                json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{
                    "name":"modifiers.list","arguments":{}
                }}),
                json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{
                    "name":"modifiers.set",
                    "arguments":{"index":0,"params":{"amount_m":120.0},"enabled":false}
                }}),
                json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{
                    "name":"modifiers.bake","arguments":{}
                }}),
                json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{
                    "name":"modifiers.list","arguments":{}
                }}),
            ],
            &workspace,
        );
        for response in &responses {
            assert_eq!(
                response["result"]["isError"], false,
                "tool failed: {response}"
            );
        }
        assert_eq!(
            responses[1]["result"]["structuredContent"]["kinds"]
                .as_array()
                .unwrap()
                .len(),
            ModifierKind::ALL.len()
        );
        assert_eq!(
            responses[4]["result"]["structuredContent"]["items"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            responses[5]["result"]["structuredContent"]["enabled"],
            false
        );
        assert_eq!(
            responses[7]["result"]["structuredContent"]["items"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        std::fs::remove_dir_all(&workspace).ok();
    }

    #[test]
    fn unknown_method_and_notification() {
        let workspace = std::env::temp_dir().join(format!("vf-mcp-unknown-{}", std::process::id()));
        std::fs::create_dir_all(&workspace).unwrap();
        let responses = run_session(
            &[
                json!({"jsonrpc":"2.0","id":1,"method":"nope","params":{}}),
                json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
                json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"nope","arguments":{}}}),
            ],
            &workspace,
        );
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["error"]["code"], -32601);
        assert_eq!(responses[1]["result"]["isError"], true);
        std::fs::remove_dir_all(&workspace).ok();
    }
}
